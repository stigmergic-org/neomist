#[cfg(target_os = "windows")]
compile_error!("neomist only supports macOS and Linux.");

mod config;
mod constants;
mod dns;
mod dns_server;
mod certs;
mod cache;
mod checkpoints;
mod ens;
mod gas;
mod http_server;
mod ipfs;
mod state;
mod tray;

use std::sync::{Arc, Mutex, mpsc};
use std::collections::VecDeque;
use std::env;

use alloy::providers::{Provider, ProviderBuilder};
use eyre::{Result, WrapErr};
use helios::ethereum::{config::networks::Network, EthereumClient, EthereumClientBuilder};
use reqwest::Url;
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::{EnvFilter, Registry, Layer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::{config_path, data_dir, load_or_create_config, save_config};
use crate::certs::CertManager;
use crate::state::AppState;

const HELIOS_RPC_ADDR: &str = "127.0.0.1:8545";

fn main() -> Result<()> {
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        return Err(eyre::eyre!("Failed to install rustls crypto provider"));
    }

    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && args[1] == "uninstall" {
        if !args.iter().any(|arg| arg == "--yes") {
            return Err(eyre::eyre!("Uninstall requires --yes"));
        }
        return uninstall();
    }

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let checkpoint_history = Arc::new(tokio::sync::RwLock::new(VecDeque::new()));
    let checkpoint_layer = checkpoints::CheckpointLayer::new(checkpoint_history.clone(), 5);
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);
    Registry::default()
        .with(fmt_layer)
        .with(checkpoint_layer)
        .init();

    info!("Starting NeoMist...");

    let config_path = config_path()?;
    info!("Config path: {}", config_path.display());
    let config = load_or_create_config(&config_path)?;
    let config = maybe_install_dns(config, &config_path)?;
    info!("Consensus RPC: {}", config.consensus_rpc);
    info!("Execution RPC: {}", config.execution_rpc);
    let data_dir = data_dir()?;
    info!("Data dir: {}", data_dir.display());
    let data_dir_for_helios = data_dir.clone();

    let cert_manager = CertManager::new(&data_dir);
    cert_manager.ensure_certs().wrap_err("Failed to create certificates")?;
    if !cert_manager
        .is_root_installed()
        .wrap_err("Failed to verify root certificate")?
    {
        cert_manager
            .install_root_cert()
            .wrap_err("Failed to install root certificate")?;
        if !cert_manager
            .is_root_installed()
            .wrap_err("Failed to verify root certificate")?
        {
            return Err(eyre::eyre!("Root certificate not installed"));
        }
    }

    let runtime = tokio::runtime::Runtime::new().wrap_err("Failed to create runtime")?;
    let handle = runtime.handle().clone();

    let _runtime_guard = runtime.enter();

    let helios_client: EthereumClient = EthereumClientBuilder::new()
        .network(Network::Mainnet)
        .consensus_rpc(config.consensus_rpc.clone())?
        .execution_rpc(config.execution_rpc.clone())?
        .load_external_fallback()
        .data_dir(data_dir_for_helios)
        .rpc_address(HELIOS_RPC_ADDR.parse()?)
        .with_file_db()
        .build()
        .wrap_err("Failed to build Helios client")?;

    let helios_client = Arc::new(helios_client);
    let http_client = reqwest::Client::new();
    let (ipfs_gateway_port, kubo_child) = runtime
        .block_on(ipfs::init_kubo(http_client.clone(), data_dir.clone()))
        .wrap_err("Failed to initialize IPFS")?;
    let kubo_child = Arc::new(Mutex::new(kubo_child));

    let ens_provider = ProviderBuilder::new()
        .connect_http(Url::parse(&format!("http://{HELIOS_RPC_ADDR}"))?)
        .erased();

    let execution_rpc_url = config.execution_rpc.clone();
    let state = AppState {
        config: Arc::new(tokio::sync::RwLock::new(config)),
        config_path,
        helios_rpc_url: format!("http://{HELIOS_RPC_ADDR}"),
        execution_rpc_url,
        ens_provider: Arc::new(ens_provider),
        http_client: http_client.clone(),
        ipfs_gateway_port,
        ipfs_api_url: "http://127.0.0.1:5001".to_string(),
        checkpoint_history,
    };

    info!("Starting DNS server on 127.0.0.1:{}", dns::dns_port());
    handle.spawn({
        let dns_port = dns::dns_port();
        async move {
            if let Err(err) = dns_server::run_dns_server(dns_port).await {
                error!("DNS server error: {err}");
                std::process::exit(1);
            }
        }
    });

    handle.spawn({
        let state = state.clone();
        let certs = std::sync::Arc::new(cert_manager);
        async move {
            if let Err(err) = http_server::run_https_server(state, certs).await {
                error!("HTTP server error: {err}");
            }
        }
    });

    let (gas_tx, gas_rx) = mpsc::channel();
    handle.spawn({
        let state = state.clone();
        async move {
            gas::poll_gas_price(
                state.http_client.clone(),
                state.execution_rpc_url.clone(),
                gas_tx,
            )
            .await;
        }
    });

    handle.spawn({
        let client = helios_client.clone();
        async move {
            if let Err(err) = client.wait_synced().await {
                warn!("Helios sync wait failed: {err}");
            } else {
                info!("Helios synced");
            }
        }
    });

    handle.spawn({
        let kubo_child = kubo_child.clone();
        async move {
            if signal::ctrl_c().await.is_ok() {
                info!("Stopping services (kubo, dns, node.localhost)");
                if let Ok(mut guard) = kubo_child.lock() {
                    if let Some(mut child) = guard.take() {
                        let _ = child.kill();
                    }
                }
                std::process::exit(0);
            }
        }
    });

    tray::run_tray(helios_client, gas_rx, kubo_child, handle)
}

fn uninstall() -> Result<()> {
    info!("Uninstalling DNS resolvers and certificates");
    dns::uninstall_dns_setup()?;
    let data_dir = data_dir()?;
    certs::uninstall_certs(&data_dir)?;
    info!("Uninstall complete");
    Ok(())
}

fn maybe_install_dns(mut config: config::AppConfig, config_path: &std::path::Path) -> Result<config::AppConfig> {
    if dns::dns_ready() {
        if !config.dns_setup_installed {
            config.dns_setup_installed = true;
            save_config(config_path, &config)?;
        }
        return Ok(config);
    }

    info!("DNS resolver setup required; prompting for installation");
    let installed = match dns::ensure_dns_setup() {
        Ok(installed) => installed,
        Err(err) => {
            warn!("DNS resolver setup failed: {err}");
            return Err(err);
        }
    };

    if !installed {
        return Err(eyre::eyre!("DNS resolver setup did not complete"));
    }

    config.dns_setup_attempted = true;
    config.dns_setup_installed = true;
    save_config(config_path, &config)?;
    Ok(config)
}
