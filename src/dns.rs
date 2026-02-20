use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use eyre::{Result, WrapErr};

use crate::constants::SYSTEMD_RESOLVED_CONF;

const DNS_PORT: u16 = 53535;

pub fn ensure_dns_setup() -> Result<bool> {
    match std::env::consts::OS {
        "macos" => ensure_macos_resolvers(),
        "linux" => ensure_linux_resolvers(),
        other => Err(eyre::eyre!("Unsupported OS for DNS setup: {other}")),
    }
}

pub fn uninstall_dns_setup() -> Result<()> {
    match std::env::consts::OS {
        "macos" => uninstall_macos_resolvers(),
        "linux" => uninstall_linux_resolvers(),
        other => Err(eyre::eyre!("Unsupported OS for DNS uninstall: {other}")),
    }
}

pub fn dns_port() -> u16 {
    DNS_PORT
}

pub fn dns_ready() -> bool {
    match std::env::consts::OS {
        "macos" => resolver_file_ok("/etc/resolver/eth") && resolver_file_ok("/etc/resolver/wei"),
        "linux" => systemd_resolver_ok(SYSTEMD_RESOLVED_CONF),
        _ => false,
    }
}

fn ensure_macos_resolvers() -> Result<bool> {
    if resolver_file_ok("/etc/resolver/eth") && resolver_file_ok("/etc/resolver/wei") {
        return Ok(true);
    }

    let script = format!(
        "mkdir -p /etc/resolver && \
        printf 'nameserver 127.0.0.1\\nport {DNS_PORT}\\n' > /etc/resolver/eth && \
        printf 'nameserver 127.0.0.1\\nport {DNS_PORT}\\n' > /etc/resolver/wei"
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

    if resolver_file_ok("/etc/resolver/eth") && resolver_file_ok("/etc/resolver/wei") {
        Ok(true)
    } else {
        Err(eyre::eyre!("Resolver install did not create valid files"))
    }
}

fn ensure_linux_resolvers() -> Result<bool> {
    if !Path::new("/etc/systemd/resolved.conf.d").exists() {
        return Err(eyre::eyre!("systemd-resolved not detected"));
    }

    let config = format!("[Resolve]\nDNS=127.0.0.1\nDomains=~eth ~wei\nDNSStubListener=no\n");

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

    if systemd_resolver_ok(SYSTEMD_RESOLVED_CONF) {
        Ok(true)
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

fn resolver_file_ok(path: &str) -> bool {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return false,
    };
    let mut has_nameserver = false;
    let mut has_port = false;
    let expected_port = format!("port {DNS_PORT}");

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

fn systemd_resolver_ok(path: &str) -> bool {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return false,
    };

    contents.contains("DNS=127.0.0.1") && contents.contains("Domains=~eth ~wei")
}
