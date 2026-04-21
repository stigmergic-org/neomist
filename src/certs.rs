use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use eyre::{Result, WrapErr};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, KeyPair, KeyUsagePurpose,
    NameConstraints, PKCS_ECDSA_P256_SHA256, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls_pemfile::{certs, pkcs8_private_keys};
use sha1::{Digest, Sha1};

use crate::constants::{CA_CERT_DIR, CA_CERT_PREFIX};

const ROOT_COMMON_NAME: &str = "NeoMist Root CA";
const INTERMEDIATE_ETH_COMMON_NAME: &str = "NeoMist Intermediate CA (ETH)";
const INTERMEDIATE_WEI_COMMON_NAME: &str = "NeoMist Intermediate CA (WEI)";
const LOCAL_UI_HOST: &str = "neomist.localhost";
const IPFS_API_HOST: &str = "ipfs.localhost";
const IPFS_GATEWAY_WILDCARD_HOST: &str = "*.ipfs.localhost";
const LOCAL_UI_HOSTS: &[&str] = &[LOCAL_UI_HOST, IPFS_API_HOST];
const LOCAL_UI_CERT_HOSTS: &[&str] = &[LOCAL_UI_HOST, IPFS_API_HOST, IPFS_GATEWAY_WILDCARD_HOST];
const NEOMIST_USER_HOME_ENV: &str = "NEOMIST_USER_HOME";
const SYSTEM_KEYCHAIN_PATH: &str = "/Library/Keychains/System.keychain";

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
            return Ok(());
        }

        cleanup_cert_files(self.cert_dir.parent().unwrap_or(&self.cert_dir))?;
        fs::create_dir_all(&self.cert_dir).wrap_err("Failed to create cert dir")?;

        let root_key_pem = generate_ec_key_pem()?;
        let root_key_path = self.cert_dir.join(".temp-root-key.pem");
        fs::write(&root_key_path, &root_key_pem).wrap_err("Failed to write temp root key")?;

        create_root_cert(&root_key_path, &self.root_cert_path)?;
        create_intermediate(
            &root_key_path,
            &self.root_cert_path,
            &self.intermediate_eth_key,
            &self.intermediate_eth_cert,
            INTERMEDIATE_ETH_COMMON_NAME,
            ".eth",
        )?;
        create_intermediate(
            &root_key_path,
            &self.root_cert_path,
            &self.intermediate_wei_key,
            &self.intermediate_wei_cert,
            INTERMEDIATE_WEI_COMMON_NAME,
            ".wei",
        )?;

        ensure_server_key(&self.server_key_path)?;

        create_leaf_cert(
            &root_key_path,
            &self.root_cert_path,
            &self.server_key_path,
            &self.ethereum_cert_path,
            LOCAL_UI_HOST,
            LOCAL_UI_CERT_HOSTS.to_vec(),
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

    pub fn install_root_cert_for_system(&self) -> Result<()> {
        match std::env::consts::OS {
            "macos" => install_root_macos_system(&self.root_cert_path),
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
        if LOCAL_UI_HOSTS.iter().any(|candidate| host == *candidate) || is_ipfs_gateway_host(&host)
        {
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

pub fn uninstall_requires_root(data_dir: &Path) -> Result<bool> {
    match std::env::consts::OS {
        "macos" => macos_system_keychain_has_root_cert(data_dir),
        _ => Ok(false),
    }
}

pub fn root_cert_path(data_dir: &Path) -> PathBuf {
    data_dir.join("certs").join("root-ca-cert.pem")
}

fn generate_ec_key_pem() -> Result<String> {
    KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
        .map(|key_pair| key_pair.serialize_pem())
        .wrap_err("Failed to generate EC key")
}

fn create_root_cert(root_key: &Path, root_cert: &Path) -> Result<()> {
    let mut params = CertificateParams::new(Vec::<String>::new());
    params.alg = &PKCS_ECDSA_P256_SHA256;
    params.key_pair = Some(load_key_pair(root_key)?);
    params.distinguished_name = neomist_distinguished_name(ROOT_COMMON_NAME);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    let cert = Certificate::from_params(params).wrap_err("Failed to build root cert")?;
    let pem = cert.serialize_pem().wrap_err("Failed to serialize root cert")?;
    fs::write(root_cert, pem).wrap_err("Failed to write root cert")?;
    Ok(())
}

fn create_intermediate(
    root_key: &Path,
    root_cert: &Path,
    key_out: &Path,
    cert_out: &Path,
    common_name: &str,
    permitted_dns: &str,
) -> Result<()> {
    let signer = load_signing_cert(root_cert, root_key)?;
    let key_pair = KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
        .wrap_err("Failed to generate intermediate key")?;
    let key_pem = key_pair.serialize_pem();

    let mut params = CertificateParams::new(Vec::<String>::new());
    params.alg = &PKCS_ECDSA_P256_SHA256;
    params.key_pair = Some(key_pair);
    params.distinguished_name = neomist_distinguished_name(common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![GeneralSubtree::DnsName(permitted_dns.to_string())],
        excluded_subtrees: Vec::new(),
    });
    params.use_authority_key_identifier_extension = true;

    let cert = Certificate::from_params(params).wrap_err("Failed to build intermediate cert")?;
    let cert_pem = cert
        .serialize_pem_with_signer(&signer)
        .wrap_err("Failed to sign intermediate cert")?;

    fs::write(key_out, key_pem).wrap_err("Failed to write intermediate key")?;
    fs::write(cert_out, cert_pem).wrap_err("Failed to write intermediate cert")?;
    Ok(())
}

fn ensure_server_key(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let key_pair = KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
        .wrap_err("Failed to generate server key")?;
    fs::write(path, key_pair.serialize_pem()).wrap_err("Failed to write server key")?;
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
    let signer = load_signing_cert(signer_cert, signer_key)?;
    let key_pair = load_key_pair(key_path)?;

    let mut params = CertificateParams::new(Vec::<String>::new());
    params.alg = &PKCS_ECDSA_P256_SHA256;
    params.key_pair = Some(key_pair);
    params.distinguished_name = neomist_distinguished_name(subject_cn);
    params.subject_alt_names = sans
        .into_iter()
        .map(|san| SanType::DnsName(san.to_string()))
        .collect();
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.use_authority_key_identifier_extension = true;

    let cert = Certificate::from_params(params).wrap_err("Failed to build leaf cert")?;
    let cert_pem = cert
        .serialize_pem_with_signer(&signer)
        .wrap_err("Failed to sign leaf cert")?;
    fs::write(cert_out, cert_pem).wrap_err("Failed to write leaf cert")?;
    Ok(())
}

fn is_ipfs_gateway_host(host: &str) -> bool {
    let Some(prefix) = host.strip_suffix(".ipfs.localhost") else {
        return false;
    };
    !prefix.is_empty() && !prefix.contains('.')
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
    let keychain = macos_login_keychain_path()?;
    let status = Command::new(security_bin())
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

fn install_root_macos_system(cert_path: &Path) -> Result<()> {
    let status = Command::new(security_bin())
        .arg("add-trusted-cert")
        .arg("-d")
        .arg("-r")
        .arg("trustRoot")
        .arg("-k")
        .arg(SYSTEM_KEYCHAIN_PATH)
        .arg(cert_path)
        .status()
        .wrap_err("Failed to install root certificate into system keychain")?;

    if !status.success() {
        return Err(eyre::eyre!(
            "System keychain root certificate install failed"
        ));
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
    let keychain = macos_login_keychain_path()?;
    Ok(
        keychain_contains_fingerprint(&keychain, &fingerprint)?
            || keychain_contains_fingerprint(SYSTEM_KEYCHAIN_PATH, &fingerprint)?
    )
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
    let keychain = macos_login_keychain_path()?;

    if cert_path.exists() {
        let fingerprint = cert_fingerprint_sha1(&cert_path)?;

        if keychain_contains_fingerprint(&keychain, &fingerprint)? {
            delete_certificate_from_keychain(&keychain, &fingerprint)
                .wrap_err("Failed to remove certificate from login keychain")?;
        }

        if keychain_contains_fingerprint(SYSTEM_KEYCHAIN_PATH, &fingerprint)? {
            delete_certificate_from_system_keychain(&fingerprint)
                .wrap_err("Failed to remove certificate from system keychain")?;
        }
    } else {
        if keychain_contains_common_name(&keychain, ROOT_COMMON_NAME)? {
            delete_certificate_by_name_from_keychain(&keychain, ROOT_COMMON_NAME)
                .wrap_err("Failed to remove certificate from login keychain")?;
        }

        if keychain_contains_common_name(SYSTEM_KEYCHAIN_PATH, ROOT_COMMON_NAME)? {
            delete_certificate_by_name_from_system_keychain(ROOT_COMMON_NAME)
                .wrap_err("Failed to remove certificate from system keychain")?;
        }
    }

    cleanup_cert_files(data_dir)?;
    Ok(())
}

fn macos_system_keychain_has_root_cert(data_dir: &Path) -> Result<bool> {
    let cert_path = root_cert_path(data_dir);
    if cert_path.exists() {
        let fingerprint = cert_fingerprint_sha1(&cert_path)?;
        keychain_contains_fingerprint(SYSTEM_KEYCHAIN_PATH, &fingerprint)
    } else {
        keychain_contains_common_name(SYSTEM_KEYCHAIN_PATH, ROOT_COMMON_NAME)
    }
}

fn delete_certificate_from_keychain(keychain: &str, fingerprint: &str) -> Result<()> {
    let status = Command::new(security_bin())
        .arg("delete-certificate")
        .arg("-t")
        .arg("-Z")
        .arg(fingerprint)
        .arg(keychain)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .wrap_err("Failed to run certificate delete command")?;

    if status.success() {
        Ok(())
    } else {
        Err(eyre::eyre!("Certificate delete command failed"))
    }
}

fn delete_certificate_by_name_from_keychain(keychain: &str, common_name: &str) -> Result<()> {
    let status = Command::new(security_bin())
        .arg("delete-certificate")
        .arg("-t")
        .arg("-c")
        .arg(common_name)
        .arg(keychain)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .wrap_err("Failed to run certificate delete command")?;

    if status.success() {
        Ok(())
    } else {
        Err(eyre::eyre!("Certificate delete command failed"))
    }
}

fn delete_certificate_from_system_keychain(fingerprint: &str) -> Result<()> {
    delete_certificate_from_keychain(SYSTEM_KEYCHAIN_PATH, fingerprint)
}

fn delete_certificate_by_name_from_system_keychain(common_name: &str) -> Result<()> {
    delete_certificate_by_name_from_keychain(SYSTEM_KEYCHAIN_PATH, common_name)
}

fn keychain_contains_fingerprint(keychain: &str, fingerprint: &str) -> Result<bool> {
    let output = Command::new(security_bin())
        .arg("find-certificate")
        .arg("-a")
        .arg("-Z")
        .arg(keychain)
        .output()
        .wrap_err("Failed to check keychain")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains(fingerprint))
}

fn keychain_contains_common_name(keychain: &str, common_name: &str) -> Result<bool> {
    let output = Command::new(security_bin())
        .arg("find-certificate")
        .arg("-a")
        .arg("-c")
        .arg(common_name)
        .arg(keychain)
        .output()
        .wrap_err("Failed to check keychain")?;
    Ok(output.status.success() && !output.stdout.is_empty())
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

fn security_bin() -> &'static str {
    "/usr/bin/security"
}

fn macos_login_keychain_path() -> Result<String> {
    if let Ok(home) = std::env::var(NEOMIST_USER_HOME_ENV) {
        if !home.is_empty() {
            return Ok(format!("{home}/Library/Keychains/login.keychain-db"));
        }
    }

    if let Ok(user) = std::env::var("SUDO_USER") {
        if !user.is_empty() && user != "root" {
            if let Ok(home) = home_dir_for_user(&user) {
                return Ok(format!("{home}/Library/Keychains/login.keychain-db"));
            }
        }
    }

    let home = std::env::var("HOME").wrap_err("HOME not set")?;
    Ok(format!("{home}/Library/Keychains/login.keychain-db"))
}

fn home_dir_for_user(user: &str) -> Result<String> {
    let output = Command::new("dscl")
        .arg(".")
        .arg("-read")
        .arg(format!("/Users/{user}"))
        .arg("NFSHomeDirectory")
        .output()
        .wrap_err("Failed to resolve user home directory")?;

    if !output.status.success() {
        return Err(eyre::eyre!("Failed to resolve user home directory"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let home = stdout
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| eyre::eyre!("Failed to parse user home directory"))?;
    Ok(home.to_string())
}

fn neomist_distinguished_name(common_name: &str) -> DistinguishedName {
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CountryName, "US");
    distinguished_name.push(DnType::StateOrProvinceName, "Local");
    distinguished_name.push(DnType::LocalityName, "Local");
    distinguished_name.push(DnType::OrganizationName, "NeoMist");
    distinguished_name.push(DnType::OrganizationalUnitName, "Development");
    distinguished_name.push(DnType::CommonName, common_name);
    distinguished_name
}

fn load_key_pair(path: &Path) -> Result<KeyPair> {
    let pem = fs::read_to_string(path)
        .wrap_err_with(|| format!("Failed to read key {}", path.display()))?;
    KeyPair::from_pem(&pem).wrap_err_with(|| format!("Failed to parse key {}", path.display()))
}

fn load_signing_cert(cert_path: &Path, key_path: &Path) -> Result<Certificate> {
    let cert_pem = fs::read_to_string(cert_path)
        .wrap_err_with(|| format!("Failed to read cert {}", cert_path.display()))?;
    let key_pair = load_key_pair(key_path)?;
    let params = CertificateParams::from_ca_cert_pem(&cert_pem, key_pair)
        .wrap_err_with(|| format!("Failed to parse signer cert {}", cert_path.display()))?;
    Certificate::from_params(params)
        .wrap_err_with(|| format!("Failed to load signer cert {}", cert_path.display()))
}
