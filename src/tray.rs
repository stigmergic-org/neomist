use std::process::{Child, Command};
use std::sync::{mpsc::Receiver, Arc, Mutex};

use eyre::{Result, WrapErr};
use helios::ethereum::EthereumClient;
use image::GenericImageView;
use tokio::runtime::Handle;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

pub fn run_tray(
    helios_client: Arc<EthereumClient>,
    gas_rx: Receiver<String>,
    kubo_child: Arc<Mutex<Option<Child>>>,
    handle: Handle,
) -> Result<()> {
    let event_loop = tao::event_loop::EventLoop::new();

    let icon = load_tray_icon()?;
    let menu = Menu::new();
    let explore_item = MenuItem::new("Explore IPFS", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    menu.append(&explore_item).wrap_err("Failed to add tray menu")?;
    menu.append(&quit_item).wrap_err("Failed to add tray menu")?;

    let tray_icon = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_icon_as_template(false)
        .build()
        .wrap_err("Failed to create tray icon")?;

    let menu_events = MenuEvent::receiver();
    let kubo_child = kubo_child.clone();

    event_loop.run(move |_event, _target, control_flow| {
        *control_flow = tao::event_loop::ControlFlow::Wait;

        if let Ok(event) = menu_events.try_recv() {
            if event.id == explore_item.id() {
                open_url("https://webui.ipfs.io");
            } else if event.id == quit_item.id() {
                let client = helios_client.clone();
                if let Ok(mut guard) = kubo_child.lock() {
                    if let Some(mut child) = guard.take() {
                        let _ = child.kill();
                    }
                }
                handle.spawn(async move {
                    client.shutdown().await;
                });
                *control_flow = tao::event_loop::ControlFlow::Exit;
                return;
            }
        }

        if let Ok(label) = gas_rx.try_recv() {
            tray_icon.set_title(Some(label));
        }
    });
}

fn load_tray_icon() -> Result<Icon> {
    let bytes = include_bytes!("../../assets/logo.png");
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
