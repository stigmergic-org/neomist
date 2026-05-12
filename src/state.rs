use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use alloy::providers::DynProvider;
use tokio::sync::RwLock;

use crate::config::AppConfig;
use crate::helios_manager::HeliosRuntime;
use crate::tray::TrayState;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub config_path: PathBuf,
    pub tray_state: Arc<TrayState>,
    pub helios_rpc_url: String,
    pub helios_has_synced: Arc<AtomicBool>,
    pub helios_runtime: Option<Arc<HeliosRuntime>>,
    pub ens_provider: Arc<DynProvider>,
    pub http_client: reqwest::Client,
    pub managed_ipfs: bool,
    pub ipfs_gateway_port: u16,
    pub ipfs_api_url: String,
    pub checkpoint_history: Arc<RwLock<VecDeque<String>>>,
}
