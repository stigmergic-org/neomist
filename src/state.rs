use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use alloy::providers::DynProvider;
use tokio::sync::RwLock;

use crate::config::AppConfig;
use crate::tray::TrayState;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub config_path: PathBuf,
    pub tray_state: Arc<TrayState>,
    pub helios_rpc_url: String,
    pub ens_provider: Arc<DynProvider>,
    pub http_client: reqwest::Client,
    pub managed_ipfs: bool,
    pub ipfs_gateway_port: u16,
    pub ipfs_api_url: String,
    pub checkpoint_history: Arc<RwLock<VecDeque<String>>>,
}
