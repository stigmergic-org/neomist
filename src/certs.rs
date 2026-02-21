use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use eyre::{Result, WrapErr};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls_pemfile::{certs, pkcs8_private_keys};
use sha1::{Digest, Sha1};

use crate::constants::{CA_CERT_DIR, CA_CERT_PREFIX};

const ROOT_SUBJECT: &str = "/C=US/ST=Local/L=Local/O=NeoMist/OU=Development/CN=NeoMist Root CA";
const INTERMEDIATE_ETH_SUBJECT: &str =
    "/C=US/ST=Local/L=Local/O=NeoMist/OU=Development/CN=NeoMist Intermediate CA (ETH)";
const INTERMEDIATE_WEI_SUBJECT: &str =
    "/C=US/ST=Local/L=Local/O=NeoMist/OU=Development/CN=NeoMist Intermediate CA (WEI)";

#[derive(Debug)]
pub struct CertManager {
    cert_dir: PathBuf,
    root_cert_path: PathBuf,
    intermediate_eth_key: PathBuf,
    intermediate_eth_cert: PathBuf,
    intermediate_wei_key: PathBuf,
    intermediate_wei_cert: PathBuf,
    server_key_path: PathBuf,
    ethereum_cert_path: PathBuf,
}

impl CertManager {
    pub fn new(data_dir: &Path) -> Self {
        let cert_dir = data_dir.join("certs");
        Self {
            root_cert_path: cert_dir.join("root-ca-cert.pem"),
            intermediate_eth_key: cert_dir.join("intermediate-eth-key.pem"),
            intermediate_eth_cert: cert_dir.join("intermediate-eth-cert.pem"),
            intermediate_wei_key: cert_dir.join("intermediate-wei-key.pem"),
            intermediate_wei_cert: cert_dir.join("intermediate-wei-cert.pem"),
            server_key_path: cert_dir.join("server-key.pem"),
            ethereum_cert_path: cert_dir.join("ethereum-cert.pem"),
            cert_dir,
        }
    }

    pub fn ensure_certs(&self) -> Result<()> {
        fs::create_dir_all(&self.cert_dir).wrap_err("Failed to create cert dir")?;

        let have_base = self.intermediate_eth_key.exists()
            && self.intermediate_eth_cert.exists()
            && self.intermediate_wei_key.exists()
            && self.intermediate_wei_cert.exists()
            && self.ethereum_cert_path.exists()
            && self.server_key_path.exists()
            && self.root_cert_path.exists();

        if have_base {
            if is_ec_key(&self.server_key_path)? {
                if leaf_key_usage_ok(&self.ethereum_cert_path)? {
                    return Ok(());
                }
            }
            cleanup_cert_files(self.cert_dir.parent().unwrap_or(&self.cert_dir))?;
        }

        let root_key_pem = generate_ec_key_pem()?;
        let root_key_path = self.cert_dir.join(".temp-root-key.pem");
        fs::write(&root_key_path, &root_key_pem).wrap_err("Failed to write temp root key")?;

        create_root_cert(&root_key_path, &self.root_cert_path)?;
        create_intermediate(
            &root_key_path,
            &self.root_cert_path,
            &self.intermediate_eth_key,
            &self.intermediate_eth_cert,
            INTERMEDIATE_ETH_SUBJECT,
            ".eth",
        )?;
        create_intermediate(
            &root_key_path,
            &self.root_cert_path,
            &self.intermediate_wei_key,
            &self.intermediate_wei_cert,
            INTERMEDIATE_WEI_SUBJECT,
            ".wei",
        )?;

        ensure_server_key(&self.server_key_path)?;

        create_leaf_cert(
            &root_key_path,
            &self.root_cert_path,
            &self.server_key_path,
            &self.ethereum_cert_path,
            "ethereum.localhost",
            vec!["ethereum.localhost"],
        )?;

        fs::remove_file(&root_key_path).wrap_err("Failed to delete temp root key")?;
        Ok(())
    }

    pub fn install_root_cert(&self) -> Result<()> {
        match std::env::consts::OS {
            "macos" => install_root_macos(&self.root_cert_path),
            "linux" => install_root_linux(&self.root_cert_path),
            other => Err(eyre::eyre!("Unsupported OS for cert install: {other}")),
        }
    }

    pub fn is_root_installed(&self) -> Result<bool> {
        match std::env::consts::OS {
            "macos" => is_root_installed_macos(&self.root_cert_path),
            "linux" => is_root_installed_linux(&self.root_cert_path),
            other => Err(eyre::eyre!("Unsupported OS for cert check: {other}")),
        }
    }

    pub fn get_chain_for_host(
        &self,
        host: &str,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let host = host.to_lowercase();
        if host == "ethereum.localhost" {
            return load_leaf_chain(
                &self.ethereum_cert_path,
                &self.root_cert_path,
                &self.server_key_path,
            );
        }

        if host.ends_with(".eth") {
            let pattern = base_domain_pattern(&host, "eth")?;
            return self.load_or_create_wildcard(&pattern, "eth");
        }

        if host.ends_with(".wei") {
            let pattern = base_domain_pattern(&host, "wei")?;
            return self.load_or_create_wildcard(&pattern, "wei");
        }

        Err(eyre::eyre!("Unsupported host for TLS: {host}"))
    }

    fn load_or_create_wildcard(
        &self,
        pattern: &str,
        tld: &str,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let cert_path = self.cert_dir.join(format!(
            "wildcard-{}-cert.pem",
            pattern.replace('.', "-").replace('*', "wildcard")
        ));

        if !cert_path.exists() {
            let (intermediate_key, intermediate_cert) = if tld == "eth" {
                (&self.intermediate_eth_key, &self.intermediate_eth_cert)
            } else {
                (&self.intermediate_wei_key, &self.intermediate_wei_cert)
            };
            let wildcard = format!("*.{pattern}");
            create_leaf_cert(
                intermediate_key,
                intermediate_cert,
                &self.server_key_path,
                &cert_path,
                pattern,
                vec![pattern, &wildcard],
            )?;
        }

        let chain = if tld == "eth" {
            vec![
                self.intermediate_eth_cert.clone(),
                self.root_cert_path.clone(),
            ]
        } else {
            vec![
                self.intermediate_wei_cert.clone(),
                self.root_cert_path.clone(),
            ]
        };

        load_leaf_chain_with_chain(&cert_path, &chain, &self.server_key_path)
    }
}

pub fn uninstall_certs(data_dir: &Path) -> Result<()> {
    match std::env::consts::OS {
        "macos" => uninstall_macos(data_dir),
        "linux" => uninstall_linux(data_dir),
        other => Err(eyre::eyre!("Unsupported OS for cert uninstall: {other}")),
    }
}

pub fn root_cert_path(data_dir: &Path) -> PathBuf {
    data_dir.join("certs").join("root-ca-cert.pem")
}

fn generate_ec_key_pem() -> Result<String> {
    let output = Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
        ])
        .output()
        .wrap_err("Failed to generate EC key")?;
    if !output.status.success() {
        return Err(eyre::eyre!("OpenSSL keygen failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn create_root_cert(root_key: &Path, root_cert: &Path) -> Result<()> {
    let ext_path = root_cert.with_extension("cnf");
    let ext = "[req]\ndistinguished_name = req_distinguished_name\nx509_extensions = v3_ca\n\n[req_distinguished_name]\n\n[v3_ca]\nbasicConstraints = critical,CA:true\nkeyUsage = critical,keyCertSign,cRLSign\nsubjectKeyIdentifier = hash\nauthorityKeyIdentifier = keyid:always,issuer:always\n";
    fs::write(&ext_path, ext).wrap_err("Failed to write root ext")?;

    let status = Command::new("openssl")
        .args(["req", "-new", "-x509", "-days", "3650", "-key"])
        .arg(root_key)
        .args(["-out"])
        .arg(root_cert)
        .args(["-subj", ROOT_SUBJECT, "-config"])
        .arg(&ext_path)
        .args(["-extensions", "v3_ca"])
        .status()
        .wrap_err("Failed to create root cert")?;

    fs::remove_file(&ext_path).ok();
    if !status.success() {
        return Err(eyre::eyre!("OpenSSL root cert failed"));
    }
    Ok(())
}

fn create_intermediate(
    root_key: &Path,
    root_cert: &Path,
    key_out: &Path,
    cert_out: &Path,
    subject: &str,
    permitted_dns: &str,
) -> Result<()> {
    let key_status = Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-out",
        ])
        .arg(key_out)
        .status()
        .wrap_err("Failed to generate intermediate key")?;
    if !key_status.success() {
        return Err(eyre::eyre!("OpenSSL intermediate key failed"));
    }

    let csr_path = cert_out.with_extension("csr");
    let csr_status = Command::new("openssl")
        .args(["req", "-new", "-key"])
        .arg(key_out)
        .args(["-out"])
        .arg(&csr_path)
        .args(["-subj", subject])
        .status()
        .wrap_err("Failed to create intermediate CSR")?;
    if !csr_status.success() {
        return Err(eyre::eyre!("OpenSSL intermediate CSR failed"));
    }

    let ext_path = cert_out.with_extension("cnf");
    let ext = format!(
        "[v3_intermediate_ca]\nbasicConstraints = critical,CA:true,pathlen:0\nkeyUsage = critical,keyCertSign,cRLSign\nsubjectKeyIdentifier = hash\nauthorityKeyIdentifier = keyid:always,issuer:always\nnameConstraints = critical,permitted;DNS:{permitted_dns}\n"
    );
    fs::write(&ext_path, ext).wrap_err("Failed to write intermediate ext")?;

    let sign_status = Command::new("openssl")
        .args(["x509", "-req", "-in"])
        .arg(&csr_path)
        .args(["-CA"])
        .arg(root_cert)
        .args(["-CAkey"])
        .arg(root_key)
        .args(["-CAcreateserial", "-out"])
        .arg(cert_out)
        .args(["-days", "3650", "-sha256", "-extfile"])
        .arg(&ext_path)
        .args(["-extensions", "v3_intermediate_ca"])
        .status()
        .wrap_err("Failed to sign intermediate")?;

    fs::remove_file(&csr_path).ok();
    fs::remove_file(&ext_path).ok();
    if !sign_status.success() {
        return Err(eyre::eyre!("OpenSSL intermediate sign failed"));
    }
    Ok(())
}

fn ensure_server_key(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let status = Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "EC",
            "-pkeyopt",
            "ec_paramgen_curve:P-256",
            "-out",
        ])
        .arg(path)
        .status()
        .wrap_err("Failed to generate server key")?;
    if !status.success() {
        return Err(eyre::eyre!("OpenSSL server key failed"));
    }
    Ok(())
}

fn create_leaf_cert(
    signer_key: &Path,
    signer_cert: &Path,
    key_path: &Path,
    cert_out: &Path,
    subject_cn: &str,
    sans: Vec<&str>,
) -> Result<()> {
    let csr_path = cert_out.with_extension("csr");
    let subject = format!("/C=US/ST=Local/L=Local/O=NeoMist/OU=Development/CN={subject_cn}");
    let csr_status = Command::new("openssl")
        .args(["req", "-new", "-key"])
        .arg(key_path)
        .args(["-out"])
        .arg(&csr_path)
        .args(["-subj", &subject])
        .status()
        .wrap_err("Failed to create leaf CSR")?;
    if !csr_status.success() {
        return Err(eyre::eyre!("OpenSSL leaf CSR failed"));
    }

    let ext_path = cert_out.with_extension("cnf");
    let mut ext = String::from("[v3_req]\nbasicConstraints = CA:FALSE\nkeyUsage = digitalSignature\nextendedKeyUsage = serverAuth\nsubjectAltName = @alt_names\n\n[alt_names]\n");
    for (idx, san) in sans.iter().enumerate() {
        ext.push_str(&format!("DNS.{} = {}\n", idx + 1, san));
    }
    fs::write(&ext_path, ext).wrap_err("Failed to write leaf ext")?;

    let sign_status = Command::new("openssl")
        .args(["x509", "-req", "-in"])
        .arg(&csr_path)
        .args(["-CA"])
        .arg(signer_cert)
        .args(["-CAkey"])
        .arg(signer_key)
        .args(["-CAcreateserial", "-out"])
        .arg(cert_out)
        .args(["-days", "365", "-sha256", "-extfile"])
        .arg(&ext_path)
        .args(["-extensions", "v3_req"])
        .status()
        .wrap_err("Failed to sign leaf")?;

    fs::remove_file(&csr_path).ok();
    fs::remove_file(&ext_path).ok();
    if !sign_status.success() {
        return Err(eyre::eyre!("OpenSSL leaf sign failed"));
    }
    Ok(())
}

fn is_ec_key(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let output = Command::new("openssl")
        .args(["pkey", "-in"])
        .arg(path)
        .args(["-text", "-noout"])
        .output()
        .wrap_err("Failed to inspect key")?;
    if !output.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains("EC Public-Key") || stdout.contains("ASN1 OID: prime256v1"))
}

fn leaf_key_usage_ok(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let output = Command::new("openssl")
        .args(["x509", "-in"])
        .arg(path)
        .args(["-text", "-noout"])
        .output()
        .wrap_err("Failed to inspect cert")?;
    if !output.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.contains("Key Encipherment")
        && stdout.contains("Extended Key Usage")
        && stdout.contains("TLS Web Server Authentication"))
}

fn base_domain_pattern(host: &str, tld: &str) -> Result<String> {
    if host == tld {
        return Err(eyre::eyre!("Unsupported bare TLD: {tld}"));
    }
    let without = host.strip_suffix(&format!(".{tld}")).unwrap_or(host);
    let parts: Vec<&str> = without.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return Err(eyre::eyre!("Invalid host for {tld}: {host}"));
    }
    let base = parts[parts.len() - 1];
    Ok(format!("{base}.{tld}"))
}

fn load_leaf_chain(
    leaf: &Path,
    root: &Path,
    key: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    load_leaf_chain_with_chain(leaf, &[root.to_path_buf()], key)
}

fn load_leaf_chain_with_chain(
    leaf: &Path,
    chain: &[PathBuf],
    key: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let leaf_bytes = fs::read(leaf).wrap_err("Failed to read leaf cert")?;
    let mut leaf_cursor = std::io::Cursor::new(leaf_bytes);
    let mut certs_list = certs(&mut leaf_cursor)
        .collect::<Result<Vec<_>, _>>()
        .wrap_err("Failed to parse leaf cert")?;

    for path in chain {
        let bytes = fs::read(path).wrap_err("Failed to read chain cert")?;
        let mut cursor = std::io::Cursor::new(bytes);
        let mut parsed = certs(&mut cursor)
            .collect::<Result<Vec<_>, _>>()
            .wrap_err("Failed to parse chain cert")?;
        certs_list.append(&mut parsed);
    }

    let key_bytes = fs::read(key).wrap_err("Failed to read key")?;
    let mut key_cursor = std::io::Cursor::new(key_bytes);
    let mut keys = pkcs8_private_keys(&mut key_cursor)
        .collect::<Result<Vec<_>, _>>()
        .wrap_err("Failed to parse key")?;
    let key = keys
        .pop()
        .ok_or_else(|| eyre::eyre!("No private key found"))?;

    Ok((certs_list, PrivateKeyDer::Pkcs8(key)))
}

fn install_root_macos(cert_path: &Path) -> Result<()> {
    let home = std::env::var("HOME").wrap_err("HOME not set")?;
    let keychain = format!("{home}/Library/Keychains/login.keychain-db");
    let status = Command::new("/usr/bin/security")
        .arg("add-trusted-cert")
        .arg("-r")
        .arg("trustRoot")
        .arg("-k")
        .arg(&keychain)
        .arg(cert_path)
        .status()
        .wrap_err("Failed to install root cert")?;
    if !status.success() {
        return Err(eyre::eyre!("Root cert install failed"));
    }
    Ok(())
}

fn install_root_linux(cert_path: &Path) -> Result<()> {
    let fingerprint = cert_fingerprint_sha1(cert_path)?.to_lowercase();
    let ca_file = format!("{CA_CERT_DIR}/{CA_CERT_PREFIX}-{fingerprint}.crt");
    let script = format!(
        "rm -f {CA_CERT_DIR}/{CA_CERT_PREFIX}-*.crt && cp '{}' '{}' && update-ca-certificates",
        cert_path.display(),
        ca_file
    );
    let status = Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .status()
        .wrap_err("Failed to install root cert")?;
    if !status.success() {
        return Err(eyre::eyre!("Root cert install failed"));
    }
    Ok(())
}

fn is_root_installed_macos(cert_path: &Path) -> Result<bool> {
    if !cert_path.exists() {
        return Ok(false);
    }
    let fingerprint = cert_fingerprint_sha1(cert_path)?;
    let home = std::env::var("HOME").wrap_err("HOME not set")?;
    let keychain = format!("{home}/Library/Keychains/login.keychain-db");
    let output = Command::new("security")
        .arg("find-certificate")
        .arg("-a")
        .arg("-Z")
        .arg(&keychain)
        .output()
        .wrap_err("Failed to check keychain")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains(&fingerprint))
}

fn is_root_installed_linux(cert_path: &Path) -> Result<bool> {
    if !cert_path.exists() {
        return Ok(false);
    }
    let fingerprint = cert_fingerprint_sha1(cert_path)?.to_lowercase();
    let path = format!("{CA_CERT_DIR}/{CA_CERT_PREFIX}-{fingerprint}.crt");
    Ok(Path::new(&path).exists())
}

fn uninstall_macos(data_dir: &Path) -> Result<()> {
    let cert_path = root_cert_path(data_dir);
    if !cert_path.exists() {
        return Ok(());
    }

    let fingerprint = cert_fingerprint_sha1(&cert_path)?;
    let home = std::env::var("HOME").wrap_err("HOME not set")?;
    let keychain = format!("{home}/Library/Keychains/login.keychain-db");

    let status = Command::new("/usr/bin/security")
        .arg("delete-certificate")
        .arg("-Z")
        .arg(&fingerprint)
        .arg(&keychain)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .wrap_err("Failed to remove certificate from login keychain")?;

    if !status.success() {
        return Err(eyre::eyre!(
            "Failed to remove certificate from login keychain"
        ));
    }

    cleanup_cert_files(data_dir)?;
    Ok(())
}

fn uninstall_linux(data_dir: &Path) -> Result<()> {
    let script = format!("rm -f {CA_CERT_DIR}/{CA_CERT_PREFIX}-*.crt && update-ca-certificates");
    let status = Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .status()
        .wrap_err("Failed to remove CA certificates")?;

    if !status.success() {
        return Err(eyre::eyre!("Failed to remove CA certificates"));
    }

    cleanup_cert_files(data_dir)?;
    Ok(())
}

fn cleanup_cert_files(data_dir: &Path) -> Result<()> {
    let cert_dir = data_dir.join("certs");
    if cert_dir.exists() {
        fs::remove_dir_all(cert_dir).wrap_err("Failed to remove cert directory")?;
    }
    Ok(())
}

fn cert_fingerprint_sha1(cert_path: &Path) -> Result<String> {
    let cert_pem = fs::read_to_string(cert_path).wrap_err("Failed to read root certificate")?;
    let base64_cert = cert_pem
        .replace("-----BEGIN CERTIFICATE-----", "")
        .replace("-----END CERTIFICATE-----", "")
        .lines()
        .collect::<String>();
    let der = STANDARD
        .decode(base64_cert.as_bytes())
        .wrap_err("Failed to decode certificate PEM")?;
    let mut hasher = Sha1::new();
    hasher.update(der);
    Ok(hex::encode(hasher.finalize()).to_uppercase())
}
