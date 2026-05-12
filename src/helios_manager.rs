use std::net::SocketAddr;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use eyre::{Result, WrapErr};
use helios::core::jsonrpc::{self, Handle as JsonRpcHandle};
use helios::ethereum::{EthereumClient, EthereumClientBuilder, config::networks::Network};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::tray::TrayState;

pub struct HeliosRuntime {
    data_dir: PathBuf,
    upstream_proxy_port: u16,
    rpc_addr: SocketAddr,
    client: RwLock<Arc<EthereumClient>>,
    rpc_handle: Mutex<Option<JsonRpcHandle>>,
    current_client_synced: AtomicBool,
    sync_epoch: AtomicU64,
    sync_notify: Notify,
    restarting: AtomicBool,
}

impl HeliosRuntime {
    pub async fn new(data_dir: PathBuf, upstream_proxy_port: u16, rpc_addr: SocketAddr) -> Result<Self> {
        let client = Arc::new(build_helios_client(&data_dir, upstream_proxy_port)?);
        let rpc_handle = start_rpc_server(&client, rpc_addr).await?;

        Ok(Self {
            data_dir,
            upstream_proxy_port,
            rpc_addr,
            client: RwLock::new(client),
            rpc_handle: Mutex::new(Some(rpc_handle)),
            current_client_synced: AtomicBool::new(false),
            sync_epoch: AtomicU64::new(0),
            sync_notify: Notify::new(),
            restarting: AtomicBool::new(false),
        })
    }

    pub fn current_client(&self) -> Arc<EthereumClient> {
        self.client
            .read()
            .expect("helios runtime client lock poisoned")
            .clone()
    }

    pub fn mark_current_client_synced(&self) {
        self.current_client_synced.store(true, Ordering::Relaxed);
        self.sync_epoch.fetch_add(1, Ordering::SeqCst);
        self.sync_notify.notify_waiters();
    }

    pub fn is_current_client_synced(&self) -> bool {
        self.current_client_synced.load(Ordering::Relaxed)
    }

    pub fn current_sync_epoch(&self) -> u64 {
        self.sync_epoch.load(Ordering::SeqCst)
    }

    pub async fn wait_for_sync_after(&self, after_epoch: u64, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, async {
            loop {
                let notified = self.sync_notify.notified();
                if self.current_client_synced.load(Ordering::Relaxed)
                    && self.sync_epoch.load(Ordering::SeqCst) > after_epoch
                {
                    return;
                }
                notified.await;
            }
        })
        .await
        .is_ok()
    }

    pub fn maybe_schedule_restart(
        self: &Arc<Self>,
        tray_state: Arc<TrayState>,
        host: &str,
        lookup_kind: &str,
        lag_secs: u64,
    ) {
        if self
            .restarting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        self.current_client_synced.store(false, Ordering::SeqCst);

        if lag_secs > 0 {
            warn!(
                "Helios is {lag_secs}s behind during {lookup_kind} lookup for {host}; scheduling in-process Helios restart"
            );
        } else {
            warn!(
                "Helios was unreachable during {lookup_kind} lookup for {host}; scheduling in-process Helios restart"
            );
        }

        tokio::spawn({
            let runtime = Arc::clone(self);
            async move {
                if let Err(err) = runtime.restart(tray_state).await {
                    warn!("Failed to restart Helios in process: {err:#}");
                }
            }
        });
    }

    async fn restart(self: Arc<Self>, tray_state: Arc<TrayState>) -> Result<()> {
        let restart_result = async {
            self.current_client_synced.store(false, Ordering::SeqCst);

            let old_rpc_handle = {
                self.rpc_handle
                    .lock()
                    .expect("helios runtime rpc handle lock poisoned")
                    .take()
            };
            if let Some(handle) = old_rpc_handle {
                let stopped = handle.clone();
                if let Err(err) = handle.stop() {
                    warn!("Helios RPC server stop request failed: {err}");
                }
                stopped.stopped().await;
            }

            let old_client = self.current_client();
            old_client.shutdown().await;

            info!("Restarting embedded Helios client");
            let new_client = Arc::new(build_helios_client(&self.data_dir, self.upstream_proxy_port)?);
            let new_rpc_handle = start_rpc_server(&new_client, self.rpc_addr).await?;

            {
                let mut client = self
                    .client
                    .write()
                    .expect("helios runtime client lock poisoned");
                *client = new_client.clone();
            }
            {
                let mut rpc_handle = self
                    .rpc_handle
                    .lock()
                    .expect("helios runtime rpc handle lock poisoned");
                *rpc_handle = Some(new_rpc_handle);
            }

            tray_state.set_helios_client(new_client.clone());
            let runtime = Arc::clone(&self);
            tokio::spawn(async move {
                match new_client.wait_synced().await {
                    Ok(()) => {
                        runtime.mark_current_client_synced();
                        runtime.restarting.store(false, Ordering::SeqCst);
                        info!("Helios restarted and synced");
                    }
                    Err(err) => {
                        runtime.restarting.store(false, Ordering::SeqCst);
                        warn!("Helios sync wait failed after restart: {err}");
                    }
                }
            });

            Ok(())
        }
        .await;

        if restart_result.is_err() {
            self.restarting.store(false, Ordering::SeqCst);
        }
        restart_result
    }
}

fn build_helios_client(data_dir: &std::path::Path, upstream_proxy_port: u16) -> Result<EthereumClient> {
    EthereumClientBuilder::new()
        .network(Network::Mainnet)
        .consensus_rpc(format!("http://127.0.0.1:{upstream_proxy_port}/consensus"))?
        .execution_rpc(format!("http://127.0.0.1:{upstream_proxy_port}/execution"))?
        .load_external_fallback()
        .data_dir(data_dir.to_path_buf())
        .with_file_db()
        .build()
        .wrap_err("Failed to build Helios client")
}

async fn start_rpc_server(client: &Arc<EthereumClient>, rpc_addr: SocketAddr) -> Result<JsonRpcHandle> {
    info!("Starting Helios RPC server on {rpc_addr}");
    jsonrpc::start(client.as_ref().deref().clone(), rpc_addr)
        .await
        .wrap_err("Failed to start Helios RPC server")
}
