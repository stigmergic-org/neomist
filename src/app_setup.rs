use std::env;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use directories::BaseDirs;
use eyre::{ContextCompat, Result, WrapErr};
use tracing::info;

use crate::certs::CertManager;
use crate::config::{AppConfig, NEOMIST_DATA_DIR_ENV, data_dir, save_config};
use crate::dns;

const APPLICATIONS_DIR: &str = "/Applications";
const CLI_LINK_PATH: &str = "/usr/local/bin/neomist";
const NEOMIST_SKIP_SYSTEM_CERT_TRUST_ENV: &str = "NEOMIST_SKIP_SYSTEM_CERT_TRUST";
const NEOMIST_REAL_USER_ENV: &str = "NEOMIST_REAL_USER";
const START_ON_LOGIN_LABEL: &str = "org.neomist.app";
const LINUX_AUTOSTART_FILE_NAME: &str = "neomist.desktop";

pub fn prepare_runtime_setup(
    mut config: AppConfig,
    config_path: &Path,
    data_dir: &Path,
) -> Result<AppConfig> {
    if std::env::consts::OS == "macos" {
        if let Some(bundle_path) = current_app_bundle()? {
            if !bundle_path.starts_with(APPLICATIONS_DIR) {
                show_alert(
                    "Move NeoMist to Applications",
                    "NeoMist must run from /Applications before it can install DNS, certificates, and CLI access.",
                );
                return Err(eyre::eyre!(
                    "NeoMist must run from /Applications before first-run setup"
                ));
            }

            if let Err(err) = ensure_local_cert_files(data_dir) {
                show_alert(
                    "NeoMist Setup Failed",
                    &format!("NeoMist could not install local certificates.\n\n{err:?}"),
                );
                return Err(err);
            }

            let cert_manager = CertManager::new(data_dir);
            let cert_trust_ready = cert_manager
                .is_root_installed()
                .wrap_err("Failed to verify root certificate")?;
            let dns_ready = dns::dns_ready();
            let cli_ready = cli_link_matches_current_exe()?;

            if !cert_trust_ready || !dns_ready || !cli_ready {
                if !show_system_setup_explainer(!dns_ready, !cert_trust_ready, !cli_ready)? {
                    return Err(eyre::eyre!("NeoMist system setup canceled by user"));
                }
            }

            if !dns_ready || !cli_ready {
                info!("macOS app setup required; prompting for administrator access");
                if let Err(err) = prompt_install_system_for_current_exe() {
                    show_alert(
                        "NeoMist Setup Incomplete",
                        &format!(
                            "NeoMist needs administrator approval to install DNS and enable CLI access.\n\n{err}"
                        ),
                    );
                    return Err(err);
                }
            }

            if !cert_manager
                .is_root_installed()
                .wrap_err("Failed to verify root certificate")?
            {
                if let Err(err) = ensure_trusted_certs(data_dir) {
                    show_alert(
                        "NeoMist Setup Incomplete",
                        &format!(
                            "NeoMist could not trust its local HTTPS certificate.\n\n{err:?}"
                        ),
                    );
                    return Err(err);
                }
            }

            if !cert_manager
                .is_root_installed()
                .wrap_err("Failed to verify root certificate")?
            {
                show_alert(
                    "NeoMist Setup Incomplete",
                    "NeoMist could not trust its local HTTPS certificate. Relaunch app and approve administrator access.",
                );
                return Err(eyre::eyre!("Root certificate not installed"));
            }

            if !dns::dns_ready() {
                show_alert(
                    "NeoMist Setup Incomplete",
                    "NeoMist could not verify DNS resolver setup. Relaunch app and approve administrator access.",
                );
                return Err(eyre::eyre!("DNS resolver setup did not complete"));
            }

            if !cli_link_matches_current_exe()? {
                show_alert(
                    "NeoMist Setup Incomplete",
                    "NeoMist could not install CLI link at /usr/local/bin/neomist.",
                );
                return Err(eyre::eyre!("CLI symlink setup did not complete"));
            }

            if !config.dns_setup_installed || !config.dns_setup_attempted {
                config.dns_setup_attempted = true;
                config.dns_setup_installed = true;
                save_config(config_path, &config)?;
            }

            return Ok(config);
        }
    }

    if std::env::consts::OS == "linux" {
        if let Err(err) = maybe_install_linux_system_integration(data_dir) {
            show_alert(
                "NeoMist Setup Incomplete",
                &format!("NeoMist could not finish Linux system setup.\n\n{err}"),
            );
            return Err(err);
        }
    }

    info!("DNS resolver setup check");
    config = maybe_install_dns(config, config_path)?;
    info!("Certificate setup check");
    ensure_trusted_certs(data_dir)?;
    Ok(config)
}

pub fn install_system_for_current_exe() -> Result<()> {
    match std::env::consts::OS {
        "macos" => install_system_for_current_exe_macos(),
        "linux" => install_system_for_current_exe_linux(),
        other => Err(eyre::eyre!("system install is not supported on {other}")),
    }
}

fn install_system_for_current_exe_macos() -> Result<()> {
    if let Some(bundle_path) = current_app_bundle()? {
        if !bundle_path.starts_with(APPLICATIONS_DIR) {
            return Err(eyre::eyre!(
                "NeoMist must run from /Applications before system integration install"
            ));
        }
    }

    let exe_path = current_exe_path()?;
    let cert_data_dir = data_dir()?;
    if std::env::var_os(NEOMIST_SKIP_SYSTEM_CERT_TRUST_ENV).is_none() {
        ensure_system_cert_trust(&cert_data_dir)?;
    }
    restore_sudo_user_data_dir_ownership(&cert_data_dir)?;
    dns::ensure_dns_setup_noninteractive()?;
    install_cli_link(&exe_path)?;
    Ok(())
}

fn install_system_for_current_exe_linux() -> Result<()> {
    let exe_path = current_exe_path()?;
    let cert_data_dir = data_dir()?;
    ensure_local_cert_files(&cert_data_dir)?;
    grant_linux_bind_service_capability(&exe_path)?;
    CertManager::new(&cert_data_dir)
        .install_root_cert_for_system()
        .wrap_err("Failed to install root certificate")?;
    restore_sudo_user_data_dir_ownership(&cert_data_dir)?;
    dns::ensure_dns_setup_noninteractive()?;
    Ok(())
}

pub fn sync_start_on_login(enabled: bool) -> Result<()> {
    match std::env::consts::OS {
        "macos" => sync_start_on_login_macos(enabled),
        "linux" => sync_start_on_login_linux(enabled),
        other => Err(eyre::eyre!("Start on login is not supported on {other}")),
    }
}

fn ensure_local_cert_files(data_dir: &Path) -> Result<()> {
    CertManager::new(data_dir)
        .ensure_certs()
        .wrap_err("Failed to create certificates")
}

fn ensure_trusted_certs(data_dir: &Path) -> Result<()> {
    let cert_manager = CertManager::new(data_dir);
    ensure_local_cert_files(data_dir)?;
    if !cert_manager
        .is_root_installed()
        .wrap_err("Failed to verify root certificate")?
    {
        cert_manager
            .install_root_cert()
            .wrap_err("Failed to install root certificate")?;
    }
    if !cert_manager
        .is_root_installed()
        .wrap_err("Failed to verify root certificate")?
    {
        return Err(eyre::eyre!("Root certificate not installed"));
    }

    Ok(())
}

fn ensure_system_cert_trust(data_dir: &Path) -> Result<()> {
    let cert_manager = CertManager::new(data_dir);
    ensure_local_cert_files(data_dir)?;
    if !cert_manager
        .is_root_installed()
        .wrap_err("Failed to verify root certificate")?
    {
        cert_manager
            .install_root_cert_for_system()
            .wrap_err("Failed to install root certificate")?;
    }
    if !cert_manager
        .is_root_installed()
        .wrap_err("Failed to verify root certificate")?
    {
        return Err(eyre::eyre!("Root certificate not installed"));
    }

    Ok(())
}

fn maybe_install_linux_system_integration(data_dir: &Path) -> Result<()> {
    let exe_path = current_exe_path()?;
    let cert_manager = CertManager::new(data_dir);
    let needs_https_bind = !current_user_is_root()? && !current_exe_has_bind_service_capability(&exe_path)?;
    let needs_dns = !dns::dns_ready();
    let needs_cert = !cert_manager
        .is_root_installed()
        .wrap_err("Failed to verify root certificate")?;

    if !needs_https_bind && !needs_dns && !needs_cert {
        return Ok(());
    }

    if !show_linux_system_setup_explainer(needs_https_bind, needs_dns, needs_cert)? {
        return Err(eyre::eyre!("NeoMist system setup canceled by user"));
    }

    prompt_install_system_for_current_exe_linux(data_dir)?;
    if needs_https_bind {
        relaunch_current_process(&exe_path)?;
    }

    Ok(())
}

fn current_exe_has_bind_service_capability(exe_path: &Path) -> Result<bool> {
    let output = Command::new("getcap")
        .arg(exe_path)
        .output()
        .wrap_err("Failed to inspect Linux file capabilities")?;

    if !output.status.success() {
        return Ok(false);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains("cap_net_bind_service"))
}

fn grant_linux_bind_service_capability(exe_path: &Path) -> Result<()> {
    let status = Command::new("setcap")
        .arg("cap_net_bind_service=+ep")
        .arg(exe_path)
        .status()
        .wrap_err(
            "Failed to grant Linux bind capability. Install libcap2-bin if setcap is unavailable.",
        )?;

    if status.success() {
        Ok(())
    } else {
        Err(eyre::eyre!(
            "Failed to grant local HTTPS access to port 443"
        ))
    }
}

fn prompt_install_system_for_current_exe_linux(data_dir: &Path) -> Result<()> {
    let exe_path = current_exe_path()?;
    let user = current_user_name()?;
    let output = Command::new("pkexec")
        .arg("/usr/bin/env")
        .arg(format!(
            "{NEOMIST_DATA_DIR_ENV}={}",
            data_dir.to_string_lossy()
        ))
        .arg(format!("{NEOMIST_REAL_USER_ENV}={user}"))
        .arg(&exe_path)
        .arg("system")
        .arg("install")
        .arg("--yes")
        .output()
        .wrap_err("Failed to prompt for administrator access")?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "unknown error".to_string()
        };
        Err(eyre::eyre!("Administrator approval required to finish NeoMist setup: {detail}"))
    }
}

fn relaunch_current_process(exe_path: &Path) -> Result<()> {
    Command::new(exe_path)
        .args(env::args_os().skip(1))
        .spawn()
        .wrap_err("Failed to relaunch NeoMist after Linux HTTPS setup")?;
    std::process::exit(0);
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

fn restore_sudo_user_data_dir_ownership(data_dir: &Path) -> Result<()> {
    let user = env::var(NEOMIST_REAL_USER_ENV)
        .ok()
        .filter(|user| !user.is_empty() && user != "root")
        .or_else(|| {
            env::var("SUDO_USER")
                .ok()
                .filter(|user| !user.is_empty() && user != "root")
        });

    let Some(user) = user else {
        return Ok(());
    };

    let Some(share_dir) = data_dir.parent() else {
        return Ok(());
    };
    let Some(local_dir) = share_dir.parent() else {
        return Ok(());
    };

    chown_path_to_user(local_dir, &user, false)?;
    chown_path_to_user(share_dir, &user, false)?;
    chown_path_to_user(data_dir, &user, true)?;
    Ok(())
}

fn chown_path_to_user(path: &Path, user: &str, recursive: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let mut command = Command::new("chown");
    if recursive {
        command.arg("-R");
    }
    let status = command
        .arg(user)
        .arg(path)
        .status()
        .wrap_err_with(|| format!("Failed to update ownership for {}", path.display()))?;

    if status.success() {
        Ok(())
    } else {
        Err(eyre::eyre!(
            "Failed to update ownership for {}",
            path.display()
        ))
    }
}

fn maybe_install_dns(mut config: AppConfig, config_path: &Path) -> Result<AppConfig> {
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
        Err(err) => return Err(err),
    };

    if !installed {
        return Err(eyre::eyre!("DNS resolver setup did not complete"));
    }

    config.dns_setup_attempted = true;
    config.dns_setup_installed = true;
    save_config(config_path, &config)?;
    Ok(config)
}

fn sync_start_on_login_macos(enabled: bool) -> Result<()> {
    let launch_agent_path = macos_launch_agent_path()?;
    if !enabled {
        return remove_file_if_exists(&launch_agent_path);
    }

    let bundle_path = current_app_bundle()?.ok_or_else(|| {
        eyre::eyre!("Start on login on macOS requires running NeoMist.app from /Applications")
    })?;
    if !bundle_path.starts_with(APPLICATIONS_DIR) {
        return Err(eyre::eyre!(
            "Start on login on macOS requires running NeoMist.app from /Applications"
        ));
    }

    if let Some(parent) = launch_agent_path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("Failed to create {}", parent.display()))?;
    }

    fs::write(&launch_agent_path, macos_launch_agent_contents(&bundle_path)).wrap_err_with(|| {
        format!(
            "Failed to write macOS launch agent at {}",
            launch_agent_path.display()
        )
    })?;
    Ok(())
}

fn sync_start_on_login_linux(enabled: bool) -> Result<()> {
    let autostart_path = linux_autostart_path()?;
    if !enabled {
        return remove_file_if_exists(&autostart_path);
    }

    let exe_path = current_exe_path()?;
    if let Some(parent) = autostart_path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("Failed to create {}", parent.display()))?;
    }

    fs::write(&autostart_path, linux_autostart_contents(&exe_path)).wrap_err_with(|| {
        format!(
            "Failed to write Linux autostart entry at {}",
            autostart_path.display()
        )
    })?;
    Ok(())
}

fn macos_launch_agent_path() -> Result<PathBuf> {
    Ok(user_home_dir()?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{START_ON_LOGIN_LABEL}.plist")))
}

fn linux_autostart_path() -> Result<PathBuf> {
    let base = BaseDirs::new().wrap_err("Failed to resolve base directories")?;
    Ok(base
        .config_dir()
        .join("autostart")
        .join(LINUX_AUTOSTART_FILE_NAME))
}

fn user_home_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().wrap_err("Failed to resolve base directories")?;
    Ok(base.home_dir().to_path_buf())
}

fn macos_launch_agent_contents(bundle_path: &Path) -> String {
    let bundle_path = xml_escape(&bundle_path.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{START_ON_LOGIN_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/open</string>
        <string>{bundle_path}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#
    )
}

fn linux_autostart_contents(exe_path: &Path) -> String {
    let exec = desktop_entry_quote(&exe_path.to_string_lossy());
    format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=NeoMist\nComment=Launch NeoMist at login\nExec={exec}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

fn desktop_entry_quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            '%' => escaped.push_str("%%"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).wrap_err_with(|| format!("Failed to remove {}", path.display())),
    }
}

fn current_exe_path() -> Result<PathBuf> {
    let exe_path = env::current_exe().wrap_err("Failed to resolve current executable")?;
    match fs::canonicalize(&exe_path) {
        Ok(path) => Ok(path),
        Err(_) => Ok(exe_path),
    }
}

fn current_app_bundle() -> Result<Option<PathBuf>> {
    let exe_path = current_exe_path()?;
    for ancestor in exe_path.ancestors() {
        if ancestor.extension().and_then(|ext| ext.to_str()) == Some("app") {
            return Ok(Some(ancestor.to_path_buf()));
        }
    }

    Ok(None)
}

fn cli_link_matches_current_exe() -> Result<bool> {
    cli_link_matches(&current_exe_path()?)
}

fn cli_link_matches(target: &Path) -> Result<bool> {
    let link_path = Path::new(CLI_LINK_PATH);
    let metadata = match fs::symlink_metadata(link_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(err).wrap_err_with(|| format!("Failed to inspect {CLI_LINK_PATH}"));
        }
    };

    let target = fs::canonicalize(target).wrap_err("Failed to canonicalize NeoMist executable")?;

    if metadata.file_type().is_symlink() {
        let symlink_target = fs::read_link(link_path)
            .wrap_err_with(|| format!("Failed to read {CLI_LINK_PATH}"))?;
        let resolved_target = if symlink_target.is_absolute() {
            symlink_target
        } else {
            link_path
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .join(symlink_target)
        };

        return match fs::canonicalize(&resolved_target) {
            Ok(resolved_target) => Ok(resolved_target == target),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err).wrap_err_with(|| {
                format!("Failed to canonicalize symlink target for {CLI_LINK_PATH}")
            }),
        };
    }

    match fs::canonicalize(link_path) {
        Ok(existing_target) => Ok(existing_target == target),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).wrap_err_with(|| format!("Failed to inspect {CLI_LINK_PATH}")),
    }
}

fn install_cli_link(target: &Path) -> Result<()> {
    let link_path = Path::new(CLI_LINK_PATH);
    let target = fs::canonicalize(target).wrap_err("Failed to canonicalize NeoMist executable")?;

    if let Ok(metadata) = fs::symlink_metadata(link_path) {
        if metadata.file_type().is_symlink() {
            fs::remove_file(link_path)
                .wrap_err_with(|| format!("Failed to remove existing {CLI_LINK_PATH}"))?;
        } else if let Ok(existing_target) = fs::canonicalize(link_path) {
            if existing_target == target {
                return Ok(());
            }
            return Err(eyre::eyre!(
                "{CLI_LINK_PATH} already exists and is not a symlink"
            ));
        } else {
            return Err(eyre::eyre!(
                "{CLI_LINK_PATH} already exists and is not a symlink"
            ));
        }
    }

    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("Failed to create {}", parent.display()))?;
    }
    symlink(&target, link_path)
        .wrap_err_with(|| format!("Failed to create {CLI_LINK_PATH} symlink"))?;

    if cli_link_matches(&target)? {
        Ok(())
    } else {
        Err(eyre::eyre!("CLI symlink setup did not complete"))
    }
}

fn prompt_install_system_for_current_exe() -> Result<()> {
    let exe_path = current_exe_path()?;
    let shell_command = format!(
        "{}=1 {} system install --yes",
        NEOMIST_SKIP_SYSTEM_CERT_TRUST_ENV,
        shell_quote(&exe_path)
    );
    let script = format!(
        "do shell script {} with administrator privileges",
        applescript_quote(&shell_command)
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .wrap_err("Failed to prompt for administrator access")?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "unknown error".to_string()
        };
        Err(eyre::eyre!("Administrator approval required to finish NeoMist setup: {detail}"))
    }
}

fn show_system_setup_explainer(needs_dns: bool, needs_cert: bool, needs_cli: bool) -> Result<bool> {
    let mut reasons = Vec::new();
    if needs_dns {
        reasons.push("install DNS resolvers for .eth and .wei");
    }
    if needs_cert {
        reasons.push("trust NeoMist local HTTPS certificate");
    }
    if needs_cli {
        reasons.push("create /usr/local/bin/neomist for CLI use");
    }
    let mut message = format!(
        "NeoMist needs setup approval for:\n\n- {}",
        reasons.join("\n- ")
    );
    if needs_cert {
        message.push_str(
            "\n\nAfter admin approval, macOS may ask once more to trust NeoMist local HTTPS certificate.",
        );
    } else {
        message.push_str("\n\nNeoMist will ask for admin approval next.");
    }
    let script = format!(
        "button returned of (display dialog {} with title {} buttons {{\"Cancel\", \"Continue\"}} default button \"Continue\" with icon caution)",
        applescript_quote(&message),
        applescript_quote("NeoMist Needs Administrator Approval")
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .wrap_err("Failed to show system setup explanation")?;

    if !output.status.success() {
        return Err(eyre::eyre!("Failed to show system setup explanation"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim() == "Continue")
}

fn show_linux_system_setup_explainer(
    needs_https_bind: bool,
    needs_dns: bool,
    needs_cert: bool,
) -> Result<bool> {
    let mut reasons = Vec::new();
    if needs_https_bind {
        reasons.push("allow NeoMist to bind local HTTPS on port 443");
    }
    if needs_dns {
        reasons.push("install DNS routing for .eth and .wei");
    }
    if needs_cert {
        reasons.push("trust NeoMist local HTTPS certificate for the system and Firefox");
    }

    let mut message = format!(
        "NeoMist needs administrator approval for:\n\n- {}",
        reasons.join("\n- ")
    );
    if needs_https_bind {
        message.push_str("\n\nNeoMist will restart once after setup so the new HTTPS permission takes effect.");
    }
    if needs_dns || needs_cert {
        message.push_str(
            "\n\nAfter setup completes, fully restart your browser so the DNS and certificate changes take effect.",
        );
    }
    message.push_str("\n\nNeoMist will ask for administrator approval next.");

    match Command::new("kdialog")
        .arg("--title")
        .arg("NeoMist Needs Administrator Approval")
        .arg("--warningcontinuecancel")
        .arg(&message)
        .status()
    {
        Ok(status) => return Ok(status.success()),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => return Ok(false),
    }

    match Command::new("zenity")
        .arg("--question")
        .arg("--ok-label=Continue")
        .arg("--cancel-label=Cancel")
        .arg("--title")
        .arg("NeoMist Needs Administrator Approval")
        .arg("--text")
        .arg(&message)
        .status()
    {
        Ok(status) => return Ok(status.success()),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => return Ok(false),
    }

    Ok(true)
}

fn show_alert(title: &str, message: &str) {
    if cfg!(target_os = "macos") {
        let script = format!(
            "display alert {} message {} as critical buttons {{\"OK\"}} default button \"OK\"",
            applescript_quote(title),
            applescript_quote(message)
        );
        let _ = Command::new("osascript").arg("-e").arg(script).status();
        return;
    }

    if cfg!(target_os = "linux") {
        if Command::new("kdialog")
            .arg("--title")
            .arg(title)
            .arg("--msgbox")
            .arg(message)
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }

        let _ = Command::new("zenity")
            .arg("--info")
            .arg("--title")
            .arg(title)
            .arg("--text")
            .arg(message)
            .status();
    }
}

fn shell_quote(path: &Path) -> String {
    shell_quote_str(&path.to_string_lossy())
}

fn current_user_name() -> Result<String> {
    if let Ok(user) = env::var("USER") {
        if !user.is_empty() {
            return Ok(user);
        }
    }

    let output = Command::new("id")
        .arg("-un")
        .output()
        .wrap_err("Failed to determine current user name")?;
    if !output.status.success() {
        return Err(eyre::eyre!("Failed to determine current user name"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn applescript_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
