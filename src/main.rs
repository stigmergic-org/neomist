#[cfg(target_os = "windows")]
compile_error!("NeoMist only supports macOS and Linux.");

mod app_setup;
mod cache;
mod certs;
mod checkpoints;
mod config;
mod constants;
mod dns;
mod dns_server;
mod ens;
mod following;
mod gas;
mod http_server;
mod ipfs;
mod state;
mod tray;
mod rpc_proxy;

use std::collections::VecDeque;
use std::env;
use std::process::Command;
use std::sync::{Arc, mpsc};

use alloy::providers::{Provider, ProviderBuilder};
use eyre::{Result, WrapErr};
use helios::ethereum::{EthereumClient, EthereumClientBuilder, config::networks::Network};
use reqwest::Url;
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::certs::CertManager;
use crate::config::{cache_dir, config_path, data_dir, load_or_create_config};
use crate::state::AppState;

const HELIOS_RPC_ADDR: &str = "127.0.0.1:8545";

const HELP_TEXT: &str = "NeoMist

Usage:
  neomist
  neomist system <install|uninstall> --yes
  neomist --help

Commands:
  system            Manage NeoMist DNS, certificates, and CLI integration

Options:
  -h, --help  Print help
";

const SYSTEM_HELP_TEXT: &str = "Manage NeoMist system integration

Usage:
  neomist system install --yes
  neomist system uninstall --yes
  neomist system --help

Subcommands:
  install     Install NeoMist certificate trust, DNS resolvers, and CLI link.
  uninstall   Remove DNS resolvers, NeoMist certificate trust, and generated certificate files.
";

const SYSTEM_INSTALL_HELP_TEXT: &str = "Install NeoMist system integration

Usage:
  neomist system install --yes

Does:
  prepares NeoMist certificate files if missing
  installs NeoMist certificate trust when command can do so
  installs DNS resolvers for .eth and .wei
  links /usr/local/bin/neomist

Options:
  --yes       Confirm certificate, DNS, and CLI installation
  -h, --help  Print help
";

const SYSTEM_UNINSTALL_HELP_TEXT: &str = "Uninstall NeoMist system integration

Usage:
  neomist system uninstall --yes

Does:
  removes DNS resolvers
  removes NeoMist certificate trust
  deletes generated certificate files

Options:
  --yes       Confirm uninstall (may require sudo)
  -h, --help  Print help
";

enum CliCommand {
    Run,
    ShowHelp(&'static str),
    SystemInstall,
    SystemUninstall,
}

enum ConfirmedCommand {
    ShowHelp(&'static str),
    Confirmed,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match parse_cli_args(&args)? {
        CliCommand::Run => {}
        CliCommand::ShowHelp(help_text) => {
            println!("{help_text}");
            return Ok(());
        }
        CliCommand::SystemInstall => return install_system(),
        CliCommand::SystemUninstall => return uninstall(),
    }

    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        return Err(eyre::eyre!("Failed to install rustls crypto provider"));
    }

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let checkpoint_cache_dir = cache_dir()?;
    let checkpoint_cache_path = checkpoints::checkpoint_cache_path(&checkpoint_cache_dir);
    let checkpoint_history = Arc::new(tokio::sync::RwLock::new(
        checkpoints::load_checkpoint_history(&checkpoint_cache_path, 5),
    ));
    let checkpoint_layer =
        checkpoints::CheckpointLayer::new(checkpoint_history.clone(), 5, checkpoint_cache_path);
    let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);
    Registry::default()
        .with(fmt_layer)
        .with(checkpoint_layer)
        .init();

    info!("Starting NeoMist...");

    let tray_state = Arc::new(tray::TrayState::new());
    let (gas_tx, gas_rx) = mpsc::channel();
    let init_state = tray_state.clone();
    let init_checkpoint_history = checkpoint_history.clone();

    std::thread::spawn(move || {
        if let Err(err) = init_services(init_state, gas_tx, init_checkpoint_history) {
            error!("Initialization failed: {err:?}");
            std::process::exit(1);
        }
    });

    tray::run_tray(gas_rx, tray_state)
}

fn parse_cli_args(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Ok(CliCommand::Run);
    }

    match args[0].as_str() {
        "-h" | "--help" | "help" => {
            if args.len() == 1 {
                Ok(CliCommand::ShowHelp(HELP_TEXT))
            } else {
                Err(eyre::eyre!(
                    "Unexpected argument after help flag: `{}`\nRun `neomist --help` for usage.",
                    args[1]
                ))
            }
        }
        "system" => parse_system_args(&args[1..]),
        unknown => Err(eyre::eyre!(
            "Unknown command or option: `{unknown}`\nRun `neomist --help` for usage."
        )),
    }
}

fn parse_system_args(args: &[String]) -> Result<CliCommand> {
    if args.is_empty() {
        return Err(eyre::eyre!(
            "System command requires subcommand.\nRun `neomist system --help` for usage."
        ));
    }

    match args[0].as_str() {
        "-h" | "--help" | "help" => Ok(CliCommand::ShowHelp(SYSTEM_HELP_TEXT)),
        "install" => match parse_confirmed_subcommand(
            &args[1..],
            SYSTEM_INSTALL_HELP_TEXT,
            "System install requires --yes",
            "system install",
        )? {
            ConfirmedCommand::ShowHelp(help_text) => Ok(CliCommand::ShowHelp(help_text)),
            ConfirmedCommand::Confirmed => Ok(CliCommand::SystemInstall),
        },
        "uninstall" => match parse_confirmed_subcommand(
            &args[1..],
            SYSTEM_UNINSTALL_HELP_TEXT,
            "System uninstall requires --yes",
            "system uninstall",
        )? {
            ConfirmedCommand::ShowHelp(help_text) => Ok(CliCommand::ShowHelp(help_text)),
            ConfirmedCommand::Confirmed => Ok(CliCommand::SystemUninstall),
        },
        unknown => Err(eyre::eyre!(
            "Unknown system subcommand: `{unknown}`\nRun `neomist system --help` for usage."
        )),
    }
}

fn parse_confirmed_subcommand(
    args: &[String],
    help_text: &'static str,
    missing_message: &'static str,
    command_name: &'static str,
) -> Result<ConfirmedCommand> {
    if args.is_empty() {
        return Err(eyre::eyre!(missing_message));
    }

    let mut confirmed = false;
    for arg in args {
        match arg.as_str() {
            "--yes" => confirmed = true,
            "-h" | "--help" | "help" => return Ok(ConfirmedCommand::ShowHelp(help_text)),
            _ => {
                return Err(eyre::eyre!(
                    "Unknown option for {command_name}: `{arg}`\nRun `neomist {command_name} --help` for usage."
                ));
            }
        }
    }

    if !confirmed {
        return Err(eyre::eyre!(missing_message));
    }

    Ok(ConfirmedCommand::Confirmed)
}

fn init_services(
    tray_state: Arc<tray::TrayState>,
    gas_tx: mpsc::Sender<String>,
    checkpoint_history: Arc<tokio::sync::RwLock<VecDeque<String>>>,
) -> Result<()> {
    info!("Init: loading configuration");
    let config_path = config_path()?;
    info!("Config path: {}", config_path.display());
    info!("Init: resolving data directory");
    let data_dir = data_dir()?;
    info!("Data dir: {}", data_dir.display());
    let config = load_or_create_config(&config_path)?;
    let config = app_setup::prepare_runtime_setup(config, &config_path, &data_dir)?;
    tray_state.set_show_gas_price(config.show_tray_gas_price);
    info!("Consensus RPCs: {:?}", config.consensus_rpcs);
    info!("Execution RPCs: {:?}", config.execution_rpcs);
    let data_dir_for_helios = data_dir.clone();

    let cert_manager = CertManager::new(&data_dir);

    info!("Init: creating async runtime");
    let runtime = tokio::runtime::Runtime::new().wrap_err("Failed to create runtime")?;
    let handle = runtime.handle().clone();
    tray_state.set_handle(handle.clone());

    let http_client = reqwest::Client::new();
    info!("Init: initializing IPFS");
    let kubo_manager = runtime
        .block_on(ipfs::init_kubo(http_client.clone(), data_dir.clone()))
        .wrap_err("Failed to initialize IPFS")?;
    let ipfs_gateway_port = kubo_manager.gateway_port();
    let kubo_manager = Arc::new(kubo_manager);
    tray_state.set_kubo_manager(kubo_manager.clone());

    let ens_provider = ProviderBuilder::new()
        .connect_http(Url::parse(&format!("http://{HELIOS_RPC_ADDR}"))?)
        .erased();

    let state = AppState {
        config: Arc::new(tokio::sync::RwLock::new(config.clone())),
        config_path: config_path.clone(),
        tray_state: tray_state.clone(),
        helios_rpc_url: format!("http://{HELIOS_RPC_ADDR}"),
        ens_provider: Arc::new(ens_provider),
        http_client: http_client.clone(),
        managed_ipfs: kubo_manager.is_managed(),
        ipfs_gateway_port,
        ipfs_api_url: "http://127.0.0.1:5001".to_string(),
        checkpoint_history,
    };

    let proxy_port = runtime
        .block_on(rpc_proxy::run_internal_proxy(state.clone()))
        .wrap_err("Failed to start internal RPC proxy")?;

    info!("Init: building Helios client");
    let helios_client: EthereumClient = runtime
        .block_on(async {
            EthereumClientBuilder::new()
                .network(Network::Mainnet)
                .consensus_rpc(format!("http://127.0.0.1:{proxy_port}/consensus"))?
                .execution_rpc(format!("http://127.0.0.1:{proxy_port}/execution"))?
                .load_external_fallback()
                .data_dir(data_dir_for_helios)
                .rpc_address(HELIOS_RPC_ADDR.parse()?)
                .with_file_db()
                .build()
        })
        .wrap_err("Failed to build Helios client")?;
    info!("Init: Helios client ready");

    let helios_client = Arc::new(helios_client);
    tray_state.set_helios_client(helios_client.clone());

    let dns_port = dns::dns_port()?;
    info!("Init: starting DNS server on 127.0.0.1:{dns_port}");
    handle.spawn({
        async move {
            if let Err(err) = dns_server::run_dns_server(dns_port).await {
                error!("DNS server error: {err:?}");
                std::process::exit(1);
            }
        }
    });

    info!("Init: starting HTTPS server");
    handle.spawn({
        let state = state.clone();
        let certs = std::sync::Arc::new(cert_manager);
        async move {
            if let Err(err) = http_server::run_https_server(state, certs).await {
                error!("HTTP server error: {err}");
            }
        }
    });

    info!("Init: starting gas polling");
    handle.spawn({
        let state = state.clone();
        async move {
            gas::poll_gas_price(
                state.clone(),
                gas_tx,
            )
            .await;
        }
    });

    info!("Init: announcing NeoMist node marker");
    handle.spawn({
        let state = state.clone();
        async move {
            if let Err(err) = ipfs::announce_node_provider_once(&state).await {
                warn!("NeoMist node marker announce failed: {err}");
            }
        }
    });

    let (sync_tx, sync_rx) = tokio::sync::mpsc::channel(1);

    info!("Init: starting Helios sync monitor");
    handle.spawn({
        let client = helios_client.clone();
        async move {
            if let Err(err) = client.wait_synced().await {
                warn!("Helios sync wait failed: {err}");
            } else {
                info!("Helios synced");
                let _ = sync_tx.send(()).await;
            }
        }
    });

    info!("Init: installing Ctrl+C handler");
    handle.spawn({
        let kubo_manager = kubo_manager.clone();
        async move {
            if signal::ctrl_c().await.is_ok() {
                info!("Stopping services (kubo, dns, node.localhost)");
                kubo_manager.stop();
                std::process::exit(0);
            }
        }
    });

    info!("Init: starting following background task");
    handle.spawn({
        let state = state.clone();
        async move {
            following::run_following_loop(state, sync_rx).await;
        }
    });

    info!("Init: background services running");
    loop {
        std::thread::park();
    }
}

fn uninstall() -> Result<()> {
    info!("Uninstalling DNS resolvers and certificates");
    let data_dir = data_dir()?;
    let running_as_root = current_user_is_root()?;
    let needs_root = running_as_root || certs::uninstall_requires_root(&data_dir)?;

    if needs_root {
        ensure_root_command("system uninstall")?;
        dns::uninstall_dns_setup_noninteractive()?;
    } else {
        dns::uninstall_dns_setup()?;
    }

    certs::uninstall_certs(&data_dir)?;
    info!("Uninstall complete");
    Ok(())
}

fn install_system() -> Result<()> {
    ensure_root_command("system install")?;
    app_setup::install_system_for_current_exe()?;
    println!("NeoMist system integration installed.");
    Ok(())
}

fn ensure_root_command(command_name: &str) -> Result<()> {
    if current_user_is_root()? {
        Ok(())
    } else {
        Err(eyre::eyre!(
            "{command_name} requires root privileges. Run it with sudo."
        ))
    }
}

fn current_user_is_root() -> Result<bool> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .wrap_err("Failed to determine current user")?;
    if !output.status.success() {
        return Err(eyre::eyre!("Failed to determine current user"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim() == "0")
}
