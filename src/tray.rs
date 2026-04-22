use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc::Receiver};
use std::time::{Duration, Instant};

use eyre::{Result, WrapErr};
use helios::ethereum::EthereumClient;
use image::GenericImageView;
#[cfg(target_os = "macos")]
use std::ffi::{c_char, c_void};
use tokio::runtime::Handle;
use tracing::warn;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::ipfs::KuboManager;

const ICON_ACTIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/logo-active.png"
));
const ICON_INACTIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/logo-inactive.png"
));

#[cfg(target_os = "macos")]
#[repr(C)]
struct ProcessSerialNumber {
    high_long_of_psn: u32,
    low_long_of_psn: u32,
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn GetCurrentProcess(psn: *mut ProcessSerialNumber) -> i32;
    fn TransformProcessType(psn: *mut ProcessSerialNumber, transform_state: u32) -> i32;
}

#[cfg(target_os = "macos")]
const K_PROCESS_TRANSFORM_TO_UI_ELEMENT_APPLICATION: u32 = 4;

#[cfg(target_os = "macos")]
type ObjcObject = *mut c_void;

#[cfg(target_os = "macos")]
type ObjcSelector = *const c_void;

#[cfg(target_os = "macos")]
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[cfg(target_os = "macos")]
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> ObjcObject;
    fn sel_registerName(name: *const c_char) -> ObjcSelector;
    fn objc_msgSend();
}

#[cfg(target_os = "macos")]
const NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY: isize = 1;

pub struct TrayState {
    helios_client: Mutex<Option<Arc<EthereumClient>>>,
    kubo_manager: Mutex<Option<Arc<KuboManager>>>,
    runtime_handle: Mutex<Option<Handle>>,
    show_gas_price: AtomicBool,
}

impl TrayState {
    pub fn new() -> Self {
        Self {
            helios_client: Mutex::new(None),
            kubo_manager: Mutex::new(None),
            runtime_handle: Mutex::new(None),
            show_gas_price: AtomicBool::new(true),
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

    pub fn set_show_gas_price(&self, show_gas_price: bool) {
        self.show_gas_price.store(show_gas_price, Ordering::Relaxed);
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

    pub fn show_gas_price(&self) -> bool {
        self.show_gas_price.load(Ordering::Relaxed)
    }
}

pub fn run_tray(gas_rx: Receiver<String>, tray_state: Arc<TrayState>) -> Result<()> {
    let event_loop = tao::event_loop::EventLoop::new();
    let mut activation_policy_applied = configure_activation_policy();

    let icon_active = load_tray_icon(ICON_ACTIVE)?;
    let icon_inactive = load_tray_icon(ICON_INACTIVE)?;
    let menu = Menu::new();
    let title_item = MenuItem::new("NeoMist", false, None);
    let separator_top = PredefinedMenuItem::separator();
    let dashboard_item = MenuItem::new("Dashboard", true, None);
    let settings_item = MenuItem::new("Settings", true, None);
    let separator_mid = PredefinedMenuItem::separator();
    let explore_item = MenuItem::new("Explore IPFS", true, None);
    let p2p_item = MenuItem::new(ipfs_networking_menu_label(false), false, None);
    let separator_settings = PredefinedMenuItem::separator();
    let separator_quit = PredefinedMenuItem::separator();
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append(&title_item)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&separator_top)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&dashboard_item)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&separator_mid)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&explore_item)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&p2p_item).wrap_err("Failed to add tray menu")?;
    menu.append(&separator_settings)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&settings_item)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&separator_quit)
        .wrap_err("Failed to add tray menu")?;
    menu.append(&quit_item)
        .wrap_err("Failed to add tray menu")?;

    let tray_icon = TrayIconBuilder::new()
        .with_icon(icon_inactive.clone())
        .with_menu(Box::new(menu))
        .with_icon_as_template(false)
        .build()
        .wrap_err("Failed to create tray icon")?;
    activation_policy_applied = activation_policy_applied || configure_activation_policy();

    let menu_events = MenuEvent::receiver();
    let tray_state = tray_state.clone();

    let mut next_tick = Instant::now();
    let mut last_networking_enabled: Option<bool> = None;
    let mut last_show_gas_price = tray_state.show_gas_price();
    let mut last_gas_price_label: Option<String> = None;
    event_loop.run(move |_event, _target, control_flow| {
        *control_flow = tao::event_loop::ControlFlow::WaitUntil(next_tick);

        if !activation_policy_applied {
            activation_policy_applied = configure_activation_policy();
        }

        let now = Instant::now();
        if now >= next_tick {
            next_tick = now + Duration::from_millis(250);

            let show_gas_price = tray_state.show_gas_price();
            let mut latest_label = None;
            while let Ok(label) = gas_rx.try_recv() {
                latest_label = Some(label);
            }
            if let Some(label) = latest_label {
                last_gas_price_label = Some(label);
                if show_gas_price {
                    tray_icon.set_title(last_gas_price_label.as_deref());
                }
            }
            if show_gas_price != last_show_gas_price {
                if show_gas_price {
                    tray_icon.set_title(last_gas_price_label.as_deref());
                } else {
                    // `tray-icon` on macOS does not clear existing text on `None`.
                    tray_icon.set_title(Some(""));
                }
                last_show_gas_price = show_gas_price;
            }

            refresh_p2p_menu(&tray_state, &p2p_item);

            let networking_enabled = resolve_networking_enabled(&tray_state);
            refresh_explore_menu(networking_enabled, &explore_item);
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
            if event.id == dashboard_item.id() {
                open_url("https://neomist.localhost");
            } else if event.id == settings_item.id() {
                open_url("https://neomist.localhost/settings");
            } else if event.id == explore_item.id() {
                open_url("https://ipfs.localhost/webui");
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
                                    p2p_item.set_text(ipfs_networking_menu_label(offline));
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

fn configure_activation_policy() -> bool {
    #[cfg(target_os = "macos")]
    {
        let process_transformed = transform_process_to_ui_element();
        let app_policy_set = set_ns_application_activation_policy();
        process_transformed || app_policy_set
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[cfg(target_os = "macos")]
fn transform_process_to_ui_element() -> bool {
    let mut psn = ProcessSerialNumber {
        high_long_of_psn: 0,
        low_long_of_psn: 0,
    };

    unsafe {
        let status = GetCurrentProcess(&mut psn);
        if status != 0 {
            warn!("Failed to get current process for Dock suppression: {status}");
            return false;
        }

        let status = TransformProcessType(&mut psn, K_PROCESS_TRANSFORM_TO_UI_ELEMENT_APPLICATION);
        if status != 0 {
            warn!("Failed to switch NeoMist to UIElement app: {status}");
            return false;
        }
    }

    true
}

#[cfg(target_os = "macos")]
fn set_ns_application_activation_policy() -> bool {
    unsafe {
        let ns_application = objc_getClass(b"NSApplication\0".as_ptr().cast());
        if ns_application.is_null() {
            warn!("Failed to resolve NSApplication class for Dock suppression");
            return false;
        }

        let shared_application = msg_send_id(
            ns_application,
            sel_registerName(b"sharedApplication\0".as_ptr().cast()),
        );
        if shared_application.is_null() {
            warn!("Failed to resolve shared NSApplication for Dock suppression");
            return false;
        }

        let applied = msg_send_bool(
            shared_application,
            sel_registerName(b"setActivationPolicy:\0".as_ptr().cast()),
            NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY,
        );
        if !applied {
            warn!("Failed to set NeoMist activation policy to accessory");
        }

        applied
    }
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_id(receiver: ObjcObject, selector: ObjcSelector) -> ObjcObject {
    let send: unsafe extern "C" fn(ObjcObject, ObjcSelector) -> ObjcObject = unsafe {
        std::mem::transmute(objc_msgSend as *const ())
    };
    unsafe { send(receiver, selector) }
}

#[cfg(target_os = "macos")]
unsafe fn msg_send_bool(receiver: ObjcObject, selector: ObjcSelector, value: isize) -> bool {
    let send: unsafe extern "C" fn(ObjcObject, ObjcSelector, isize) -> i8 = unsafe {
        std::mem::transmute(objc_msgSend as *const ())
    };
    unsafe { send(receiver, selector, value) != 0 }
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

fn ipfs_networking_menu_label(offline: bool) -> &'static str {
    if offline {
        "Enable IPFS networking"
    } else {
        "Disable IPFS networking"
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
                p2p_item.set_text(ipfs_networking_menu_label(kubo_manager.is_offline()));
            } else {
                p2p_item.set_enabled(false);
                p2p_item.set_text("Using external IPFS instance");
            }
        }
        None => {
            p2p_item.set_enabled(false);
        }
    }
}

fn refresh_explore_menu(networking_enabled: bool, explore_item: &MenuItem) {
    explore_item.set_enabled(networking_enabled);
}
