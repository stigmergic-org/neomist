use std::env;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use eyre::{Result, WrapErr};
use tracing::info;

use crate::certs::CertManager;
use crate::config::{AppConfig, save_config};
use crate::dns;

const APPLICATIONS_DIR: &str = "/Applications";
const CLI_LINK_PATH: &str = "/usr/local/bin/neomist";

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

    info!("DNS resolver setup check");
    config = maybe_install_dns(config, config_path)?;
    info!("Certificate setup check");
    ensure_trusted_certs(data_dir)?;
    Ok(config)
}

pub fn install_system_for_current_exe() -> Result<()> {
    if std::env::consts::OS != "macos" {
        return Err(eyre::eyre!("install-system is only supported on macOS"));
    }

    if let Some(bundle_path) = current_app_bundle()? {
        if !bundle_path.starts_with(APPLICATIONS_DIR) {
            return Err(eyre::eyre!(
                "NeoMist must run from /Applications before system integration install"
            ));
        }
    }

    let exe_path = current_exe_path()?;
    dns::ensure_dns_setup_noninteractive()?;
    install_cli_link(&exe_path)?;
    Ok(())
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
    let shell_command = format!("{} install-system --yes", shell_quote(&exe_path));
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

fn show_alert(title: &str, message: &str) {
    let script = format!(
        "display alert {} message {} as critical buttons {{\"OK\"}} default button \"OK\"",
        applescript_quote(title),
        applescript_quote(message)
    );
    let _ = Command::new("osascript").arg("-e").arg(script).status();
}

fn shell_quote(path: &Path) -> String {
    shell_quote_str(&path.to_string_lossy())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn applescript_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
