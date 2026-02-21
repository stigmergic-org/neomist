use std::process::Command;
use std::sync::{mpsc::Receiver, Arc, Mutex};
use std::time::{Duration, Instant};

use eyre::{Result, WrapErr};
use helios::ethereum::EthereumClient;
use image::GenericImageView;
use tokio::runtime::Handle;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use tracing::warn;

use crate::ipfs::KuboManager;

const ICON_ACTIVE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo-active.png"));
const ICON_INACTIVE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo-inactive.png"));

pub struct TrayState {
    helios_client: Mutex<Option<Arc<EthereumClient>>>,
    kubo_manager: Mutex<Option<Arc<KuboManager>>>,
    runtime_handle: Mutex<Option<Handle>>,
}

impl TrayState {
    pub fn new() -> Self {
        Self {
            helios_client: Mutex::new(None),
            kubo_manager: Mutex::new(None),
            runtime_handle: Mutex::new(None),
        }
    }

    pub fn set_helios_client(&self, client: Arc<EthereumClient>) {
        if let Ok(mut guard) = self.helios_client.lock() {
            *guard = Some(client);
        }
    }

    pub fn set_kubo_manager(&self, manager: Arc<KuboManager>) {
        if let Ok(mut guard) = self.kubo_manager.lock() {
            *guard = Some(manager);
        }
    }

    pub fn set_handle(&self, handle: Handle) {
        if let Ok(mut guard) = self.runtime_handle.lock() {
            *guard = Some(handle);
        }
    }

    fn helios_client(&self) -> Option<Arc<EthereumClient>> {
        self.helios_client
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone))
    }

    fn kubo_manager(&self) -> Option<Arc<KuboManager>> {
        self.kubo_manager
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone))
    }

    fn handle(&self) -> Option<Handle> {
        self.runtime_handle
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(Handle::clone))
    }
}

pub fn run_tray(
    gas_rx: Receiver<String>,
    tray_state: Arc<TrayState>,
) -> Result<()> {
    let event_loop = tao::event_loop::EventLoop::new();

    let icon_active = load_tray_icon(ICON_ACTIVE)?;
    let icon_inactive = load_tray_icon(ICON_INACTIVE)?;
    let menu = Menu::new();
    let explore_item = MenuItem::new("Explore IPFS", true, None);
    let p2p_item = MenuItem::new(p2p_menu_label(false), false, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append(&explore_item).wrap_err("Failed to add tray menu")?;
    menu.append(&p2p_item).wrap_err("Failed to add tray menu")?;
    menu.append(&quit_item).wrap_err("Failed to add tray menu")?;

    let tray_icon = TrayIconBuilder::new()
        .with_icon(icon_inactive.clone())
        .with_menu(Box::new(menu))
        .with_icon_as_template(false)
        .build()
        .wrap_err("Failed to create tray icon")?;

    let menu_events = MenuEvent::receiver();
    let tray_state = tray_state.clone();

    let mut next_tick = Instant::now();
    let mut last_networking_enabled: Option<bool> = None;
    event_loop.run(move |_event, _target, control_flow| {
        *control_flow = tao::event_loop::ControlFlow::WaitUntil(next_tick);

        let now = Instant::now();
        if now >= next_tick {
            next_tick = now + Duration::from_millis(250);

            let mut latest_label = None;
            while let Ok(label) = gas_rx.try_recv() {
                latest_label = Some(label);
            }
            if let Some(label) = latest_label {
                tray_icon.set_title(Some(label));
            }

            refresh_p2p_menu(&tray_state, &p2p_item);

            let networking_enabled = resolve_networking_enabled(&tray_state);
            if last_networking_enabled != Some(networking_enabled) {
                let icon = if networking_enabled {
                    icon_active.clone()
                } else {
                    icon_inactive.clone()
                };
                if let Err(err) = tray_icon.set_icon(Some(icon)) {
                    warn!("Failed to update tray icon: {err}");
                } else {
                    last_networking_enabled = Some(networking_enabled);
                }
            }
        }

        if let Ok(event) = menu_events.try_recv() {
            if event.id == explore_item.id() {
                open_url("https://webui.ipfs.io");
            } else if event.id == p2p_item.id() {
                match tray_state.kubo_manager() {
                    Some(kubo_manager) => {
                        if !kubo_manager.is_managed() {
                            warn!("IPFS networking toggle unavailable (external IPFS)");
                            return;
                        }
                        let offline = !kubo_manager.is_offline();
                        match kubo_manager.set_offline(offline) {
                            Ok(changed) => {
                                if changed {
                                    p2p_item.set_text(p2p_menu_label(offline));
                                }
                            }
                            Err(err) => {
                                warn!("Failed to toggle IPFS networking: {err}");
                            }
                        }
                    }
                    None => {
                        warn!("IPFS is still starting; networking toggle unavailable");
                    }
                }
            } else if event.id == quit_item.id() {
                if let Some(kubo_manager) = tray_state.kubo_manager() {
                    kubo_manager.stop();
                }
                if let (Some(client), Some(handle)) =
                    (tray_state.helios_client(), tray_state.handle())
                {
                    handle.spawn(async move {
                        client.shutdown().await;
                    });
                }
                *control_flow = tao::event_loop::ControlFlow::Exit;
                return;
            }
        }

    });
}

fn load_tray_icon(bytes: &[u8]) -> Result<Icon> {
    let image = image::load_from_memory(bytes).wrap_err("Failed to decode tray icon")?;
    let rgba = image.to_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height).wrap_err("Failed to create tray icon")
}

fn open_url(url: &str) {
    let command = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    let _ = Command::new(command).arg(url).spawn();
}

fn p2p_menu_label(offline: bool) -> &'static str {
    if offline {
        "Enable P2P networking"
    } else {
        "Disable P2P networking"
    }
}

fn resolve_networking_enabled(tray_state: &TrayState) -> bool {
    match tray_state.kubo_manager() {
        Some(kubo_manager) => {
            if kubo_manager.is_managed() {
                !kubo_manager.is_offline()
            } else {
                true
            }
        }
        None => false,
    }
}

fn refresh_p2p_menu(tray_state: &TrayState, p2p_item: &MenuItem) {
    match tray_state.kubo_manager() {
        Some(kubo_manager) => {
            if kubo_manager.is_managed() {
                p2p_item.set_enabled(true);
                p2p_item.set_text(p2p_menu_label(kubo_manager.is_offline()));
            } else {
                p2p_item.set_enabled(false);
            }
        }
        None => {
            p2p_item.set_enabled(false);
        }
    }
}
