use std::fs;
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use eyre::{Result, WrapErr};

use crate::config::data_dir;
use crate::constants::SYSTEMD_RESOLVED_CONF;

const DEFAULT_DNS_PORT: u16 = 53535;
const DNS_PORT_FILE_NAME: &str = "dns-port";

pub fn ensure_dns_setup() -> Result<bool> {
    let dns_port = selected_dns_port_for_install()?;
    match std::env::consts::OS {
        "macos" => {
            let installed = ensure_macos_resolvers(dns_port)?;
            if installed {
                persist_dns_port(dns_port)?;
            }
            Ok(installed)
        }
        "linux" => {
            let installed = ensure_linux_resolvers(dns_port)?;
            if installed {
                persist_dns_port(dns_port)?;
            }
            Ok(installed)
        }
        other => Err(eyre::eyre!("Unsupported OS for DNS setup: {other}")),
    }
}

pub fn ensure_dns_setup_noninteractive() -> Result<()> {
    let dns_port = selected_dns_port_for_install()?;
    match std::env::consts::OS {
        "macos" => ensure_macos_resolvers_noninteractive(dns_port)?,
        "linux" => ensure_linux_resolvers_noninteractive(dns_port)?,
        other => return Err(eyre::eyre!("Unsupported OS for DNS setup: {other}")),
    }

    persist_dns_port(dns_port)
}

pub fn uninstall_dns_setup() -> Result<()> {
    match std::env::consts::OS {
        "macos" => uninstall_macos_resolvers()?,
        "linux" => uninstall_linux_resolvers()?,
        other => return Err(eyre::eyre!("Unsupported OS for DNS uninstall: {other}")),
    }

    remove_persisted_dns_port()
}

pub fn uninstall_dns_setup_noninteractive() -> Result<()> {
    match std::env::consts::OS {
        "macos" => uninstall_macos_resolvers_noninteractive()?,
        "linux" => uninstall_linux_resolvers_noninteractive()?,
        other => return Err(eyre::eyre!("Unsupported OS for DNS uninstall: {other}")),
    }

    remove_persisted_dns_port()
}

pub fn dns_port() -> Result<u16> {
    Ok(load_persisted_dns_port()?.unwrap_or(DEFAULT_DNS_PORT))
}

pub fn dns_ready() -> bool {
    let dns_port = match dns_port() {
        Ok(dns_port) => dns_port,
        Err(_) => return false,
    };

    match std::env::consts::OS {
        "macos" => macos_resolvers_ok(dns_port),
        "linux" => systemd_resolver_ok(Path::new(SYSTEMD_RESOLVED_CONF), dns_port),
        _ => false,
    }
}

fn ensure_macos_resolvers(dns_port: u16) -> Result<bool> {
    if macos_resolvers_ok(dns_port) {
        return Ok(true);
    }

    let script = format!(
        "mkdir -p /etc/resolver && \
        printf 'nameserver 127.0.0.1\\nport {dns_port}\\n' > /etc/resolver/eth && \
        printf 'nameserver 127.0.0.1\\nport {dns_port}\\n' > /etc/resolver/wei"
    );

    let status = Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "do shell script \"{}\" with administrator privileges",
            script
        ))
        .status()
        .wrap_err("Failed to run resolver installer")?;

    if !status.success() {
        return Err(eyre::eyre!("Resolver install failed"));
    }

    if macos_resolvers_ok(dns_port) {
        Ok(true)
    } else {
        Err(eyre::eyre!("Resolver install did not create valid files"))
    }
}

fn ensure_macos_resolvers_noninteractive(dns_port: u16) -> Result<()> {
    if macos_resolvers_ok(dns_port) {
        return Ok(());
    }

    let resolver_contents = format!("nameserver 127.0.0.1\nport {dns_port}\n");
    fs::create_dir_all("/etc/resolver").wrap_err("Failed to create /etc/resolver")?;
    fs::write("/etc/resolver/eth", &resolver_contents)
        .wrap_err("Failed to write /etc/resolver/eth")?;
    fs::write("/etc/resolver/wei", &resolver_contents)
        .wrap_err("Failed to write /etc/resolver/wei")?;

    if macos_resolvers_ok(dns_port) {
        Ok(())
    } else {
        Err(eyre::eyre!("Resolver install did not create valid files"))
    }
}

fn ensure_linux_resolvers(dns_port: u16) -> Result<bool> {
    if !Path::new("/etc/systemd/resolved.conf.d").exists() {
        return Err(eyre::eyre!("systemd-resolved not detected"));
    }

    if systemd_resolver_ok(Path::new(SYSTEMD_RESOLVED_CONF), dns_port) {
        return Ok(true);
    }

    let config = format!("[Resolve]\nDNS=127.0.0.1:{dns_port}\nDomains=~eth ~wei\nDNSStubListener=no\n");

    let script = format!(
        "mkdir -p /etc/systemd/resolved.conf.d && \
        printf '{}' > {SYSTEMD_RESOLVED_CONF} && \
        systemctl restart systemd-resolved",
        config.replace('\n', "\\n")
    );

    let status = Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .status()
        .wrap_err("Failed to run resolver installer")?;

    if !status.success() {
        return Err(eyre::eyre!("Resolver install failed"));
    }

    if systemd_resolver_ok(Path::new(SYSTEMD_RESOLVED_CONF), dns_port) {
        Ok(true)
    } else {
        Err(eyre::eyre!("Resolver install did not create valid config"))
    }
}

fn ensure_linux_resolvers_noninteractive(dns_port: u16) -> Result<()> {
    if !Path::new("/etc/systemd/resolved.conf.d").exists() {
        return Err(eyre::eyre!("systemd-resolved not detected"));
    }

    if systemd_resolver_ok(Path::new(SYSTEMD_RESOLVED_CONF), dns_port) {
        return Ok(());
    }

    let config = format!("[Resolve]\nDNS=127.0.0.1:{dns_port}\nDomains=~eth ~wei\nDNSStubListener=no\n");
    fs::create_dir_all("/etc/systemd/resolved.conf.d")
        .wrap_err("Failed to create systemd-resolved config directory")?;
    fs::write(SYSTEMD_RESOLVED_CONF, config)
        .wrap_err("Failed to write systemd-resolved config")?;
    restart_systemd_resolved()?;

    if systemd_resolver_ok(Path::new(SYSTEMD_RESOLVED_CONF), dns_port) {
        Ok(())
    } else {
        Err(eyre::eyre!("Resolver install did not create valid config"))
    }
}

fn uninstall_macos_resolvers() -> Result<()> {
    let script = "rm -f /etc/resolver/eth /etc/resolver/wei";
    let status = Command::new("osascript")
        .arg("-e")
        .arg(format!(
            "do shell script \"{}\" with administrator privileges",
            script
        ))
        .status()
        .wrap_err("Failed to run resolver uninstall")?;

    if !status.success() {
        return Err(eyre::eyre!("Resolver uninstall failed"));
    }
    Ok(())
}

fn uninstall_macos_resolvers_noninteractive() -> Result<()> {
    remove_file_if_exists(Path::new("/etc/resolver/eth"))?;
    remove_file_if_exists(Path::new("/etc/resolver/wei"))?;
    Ok(())
}

fn uninstall_linux_resolvers() -> Result<()> {
    let script = format!("rm -f {SYSTEMD_RESOLVED_CONF} && systemctl restart systemd-resolved");
    let status = Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .status()
        .wrap_err("Failed to run resolver uninstall")?;

    if !status.success() {
        return Err(eyre::eyre!("Resolver uninstall failed"));
    }
    Ok(())
}

fn uninstall_linux_resolvers_noninteractive() -> Result<()> {
    remove_file_if_exists(Path::new(SYSTEMD_RESOLVED_CONF))?;
    restart_systemd_resolved()?;
    Ok(())
}

fn restart_systemd_resolved() -> Result<()> {
    let status = Command::new("systemctl")
        .arg("restart")
        .arg("systemd-resolved")
        .stdin(Stdio::null())
        .status()
        .wrap_err("Failed to restart systemd-resolved")?;

    if status.success() {
        Ok(())
    } else {
        Err(eyre::eyre!("Failed to restart systemd-resolved"))
    }
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).wrap_err_with(|| format!("Failed to remove {}", path.display())),
    }
}

fn macos_resolvers_ok(dns_port: u16) -> bool {
    resolver_file_ok(Path::new("/etc/resolver/eth"), dns_port)
        && resolver_file_ok(Path::new("/etc/resolver/wei"), dns_port)
}

fn resolver_file_ok(path: &Path, dns_port: u16) -> bool {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return false,
    };

    resolver_contents_ok(&contents, dns_port)
}

fn resolver_contents_ok(contents: &str, dns_port: u16) -> bool {
    let mut has_nameserver = false;
    let mut has_port = false;
    let expected_port = format!("port {dns_port}");

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed == "nameserver 127.0.0.1" {
            has_nameserver = true;
        }
        if trimmed == expected_port {
            has_port = true;
        }
    }

    has_nameserver && has_port
}

fn systemd_resolver_ok(path: &Path, dns_port: u16) -> bool {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return false,
    };

    contents.contains(&format!("DNS=127.0.0.1:{dns_port}"))
        && contents.contains("Domains=~eth ~wei")
}

fn selected_dns_port_for_install() -> Result<u16> {
    match load_persisted_dns_port().ok().flatten() {
        Some(dns_port) if dns_port_is_available(dns_port) => Ok(dns_port),
        Some(_) => pick_available_dns_port(),
        None if dns_port_is_available(DEFAULT_DNS_PORT) => Ok(DEFAULT_DNS_PORT),
        None => pick_available_dns_port(),
    }
}

fn dns_port_is_available(dns_port: u16) -> bool {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, dns_port)).is_ok()
}

fn pick_available_dns_port() -> Result<u16> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .wrap_err("Failed to allocate DNS UDP port")?;
    let dns_port = socket
        .local_addr()
        .wrap_err("Failed to read allocated DNS UDP port")?
        .port();

    if dns_port == 0 {
        Err(eyre::eyre!("Allocated invalid DNS UDP port 0"))
    } else {
        Ok(dns_port)
    }
}

fn dns_port_file_path() -> Result<PathBuf> {
    Ok(data_dir()?.join(DNS_PORT_FILE_NAME))
}

fn load_persisted_dns_port() -> Result<Option<u16>> {
    let path = dns_port_file_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&path)
        .wrap_err_with(|| format!("Failed to read {}", path.display()))?;
    let trimmed = contents.trim();
    let dns_port = trimmed.parse::<u16>().wrap_err_with(|| {
        format!("Failed to parse DNS port from {}", path.display())
    })?;

    if dns_port == 0 {
        Err(eyre::eyre!("DNS port file {} contains port 0", path.display()))
    } else {
        Ok(Some(dns_port))
    }
}

fn persist_dns_port(dns_port: u16) -> Result<()> {
    let path = dns_port_file_path()?;
    fs::write(&path, format!("{dns_port}\n"))
        .wrap_err_with(|| format!("Failed to write {}", path.display()))
}

fn remove_persisted_dns_port() -> Result<()> {
    let path = dns_port_file_path()?;
    remove_file_if_exists(&path)
}
