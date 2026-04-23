use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use tracing::warn;

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
const FIREFOX_POLICIES_PATH: &str = "/etc/firefox/policies/policies.json";
const FIREFOX_POLICY_DEVICE_NAME: &str = "NeoMist System Trust";
const CERT_DIR_MODE: u32 = 0o700;
const PRIVATE_KEY_MODE: u32 = 0o600;
const CERT_FILE_MODE: u32 = 0o644;
const CERT_SCHEMA_VERSION: &str = "2";

static SERIAL_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct CertManager {
    cert_dir: PathBuf,
    schema_version_path: PathBuf,
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
            schema_version_path: cert_dir.join("version"),
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
        best_effort_set_existing_path_mode(&self.cert_dir, CERT_DIR_MODE)
            .wrap_err("Failed to secure cert directory permissions")?;

        let have_base = self.intermediate_eth_key.exists()
            && self.intermediate_eth_cert.exists()
            && self.intermediate_wei_key.exists()
            && self.intermediate_wei_cert.exists()
            && self.ethereum_cert_path.exists()
            && self.server_key_path.exists()
            && self.root_cert_path.exists()
            && cert_schema_matches(&self.schema_version_path)?;

        if have_base {
            self.harden_permissions()?;
            return Ok(());
        }

        cleanup_cert_files(self.cert_dir.parent().unwrap_or(&self.cert_dir))?;
        fs::create_dir_all(&self.cert_dir).wrap_err("Failed to create cert dir")?;
        set_path_mode(&self.cert_dir, CERT_DIR_MODE)
            .wrap_err("Failed to secure cert directory permissions")?;

        let root_cert = create_root_cert(&self.root_cert_path)?;
        create_intermediate(
            &root_cert,
            &self.intermediate_eth_key,
            &self.intermediate_eth_cert,
            INTERMEDIATE_ETH_COMMON_NAME,
            ".eth",
        )?;
        create_intermediate(
            &root_cert,
            &self.intermediate_wei_key,
            &self.intermediate_wei_cert,
            INTERMEDIATE_WEI_COMMON_NAME,
            ".wei",
        )?;

        ensure_server_key(&self.server_key_path)?;

        create_leaf_cert_with_signer(
            &root_cert,
            &self.server_key_path,
            &self.ethereum_cert_path,
            LOCAL_UI_HOST,
            LOCAL_UI_CERT_HOSTS.to_vec(),
        )?;
        write_pem_file(
            &self.schema_version_path,
            CERT_SCHEMA_VERSION,
            CERT_FILE_MODE,
        )
        .wrap_err("Failed to write cert schema version")?;

        self.harden_permissions()?;
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

    fn harden_permissions(&self) -> Result<()> {
        best_effort_set_existing_path_mode(&self.cert_dir, CERT_DIR_MODE)?;

        for path in [
            &self.intermediate_eth_key,
            &self.intermediate_wei_key,
            &self.server_key_path,
        ] {
            if path.exists() {
                best_effort_set_existing_path_mode(path, PRIVATE_KEY_MODE)?;
            }
        }

        for path in [
            &self.root_cert_path,
            &self.intermediate_eth_cert,
            &self.intermediate_wei_cert,
            &self.ethereum_cert_path,
            &self.schema_version_path,
        ] {
            if path.exists() {
                best_effort_set_existing_path_mode(path, CERT_FILE_MODE)?;
            }
        }

        for entry in fs::read_dir(&self.cert_dir).wrap_err("Failed to read cert directory")? {
            let entry = entry.wrap_err("Failed to inspect cert directory entry")?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };

            if file_name.starts_with("wildcard-") && file_name.ends_with("-cert.pem") {
                best_effort_set_existing_path_mode(&path, CERT_FILE_MODE)?;
            }
        }

        Ok(())
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

fn generate_ec_key_pair() -> Result<KeyPair> {
    KeyPair::generate(&PKCS_ECDSA_P256_SHA256)
        .wrap_err("Failed to generate EC key")
}

fn create_root_cert(root_cert: &Path) -> Result<Certificate> {
    let mut params = CertificateParams::new(Vec::<String>::new());
    params.alg = &PKCS_ECDSA_P256_SHA256;
    params.key_pair = Some(generate_ec_key_pair()?);
    params.distinguished_name = neomist_distinguished_name(ROOT_COMMON_NAME);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.serial_number = Some(fresh_serial_number(&[ROOT_COMMON_NAME]));

    let cert = Certificate::from_params(params).wrap_err("Failed to build root cert")?;
    let pem = cert.serialize_pem().wrap_err("Failed to serialize root cert")?;
    write_pem_file(root_cert, &pem, CERT_FILE_MODE).wrap_err("Failed to write root cert")?;
    Ok(cert)
}

fn create_intermediate(
    signer: &Certificate,
    key_out: &Path,
    cert_out: &Path,
    common_name: &str,
    permitted_dns: &str,
) -> Result<()> {
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
    params.serial_number = Some(fresh_serial_number(&[common_name, permitted_dns]));

    let cert = Certificate::from_params(params).wrap_err("Failed to build intermediate cert")?;
    let cert_pem = cert
        .serialize_pem_with_signer(signer)
        .wrap_err("Failed to sign intermediate cert")?;

    write_pem_file(key_out, &key_pem, PRIVATE_KEY_MODE)
        .wrap_err("Failed to write intermediate key")?;
    write_pem_file(cert_out, &cert_pem, CERT_FILE_MODE)
        .wrap_err("Failed to write intermediate cert")?;
    Ok(())
}

fn ensure_server_key(path: &Path) -> Result<()> {
    if path.exists() {
        set_path_mode(path, PRIVATE_KEY_MODE).wrap_err("Failed to secure server key permissions")?;
        return Ok(());
    }
    let key_pair = generate_ec_key_pair().wrap_err("Failed to generate server key")?;
    write_pem_file(path, &key_pair.serialize_pem(), PRIVATE_KEY_MODE)
        .wrap_err("Failed to write server key")?;
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
    create_leaf_cert_with_signer(&signer, key_path, cert_out, subject_cn, sans)
}

fn create_leaf_cert_with_signer(
    signer: &Certificate,
    key_path: &Path,
    cert_out: &Path,
    subject_cn: &str,
    sans: Vec<&str>,
) -> Result<()> {
    let key_pair = load_key_pair(key_path)?;

    let mut params = CertificateParams::new(Vec::<String>::new());
    params.alg = &PKCS_ECDSA_P256_SHA256;
    params.key_pair = Some(key_pair);
    params.distinguished_name = neomist_distinguished_name(subject_cn);
    params.serial_number = Some(fresh_serial_number(&[subject_cn]));
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
        .serialize_pem_with_signer(signer)
        .wrap_err("Failed to sign leaf cert")?;
    write_pem_file(cert_out, &cert_pem, CERT_FILE_MODE).wrap_err("Failed to write leaf cert")?;
    Ok(())
}

fn write_pem_file(path: &Path, contents: &str, mode: u32) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(mode)
        .open(path)
        .wrap_err_with(|| format!("Failed to open {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .wrap_err_with(|| format!("Failed to write {}", path.display()))?;
    set_path_mode(path, mode).wrap_err_with(|| format!("Failed to secure {}", path.display()))?;
    Ok(())
}

fn set_path_mode(path: &Path, mode: u32) -> Result<()> {
    let permissions = fs::Permissions::from_mode(mode);
    fs::set_permissions(path, permissions)
        .wrap_err_with(|| format!("Failed to set permissions on {}", path.display()))
}

fn best_effort_set_existing_path_mode(path: &Path, mode: u32) -> Result<()> {
    match set_path_mode(path, mode) {
        Ok(()) => Ok(()),
        Err(err) if path.exists() && is_permission_denied(&err) => {
            warn!(
                "Skipping permission hardening for {}: {err}",
                path.display()
            );
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn is_permission_denied(err: &eyre::Report) -> bool {
    err.chain().any(|source| {
        source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::PermissionDenied)
    })
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
    let firefox_policy = render_linux_firefox_policy_with_neomist(
        existing_linux_firefox_policy()?.as_deref(),
        &ca_file,
    )?;
    let temp_policy_path = write_linux_firefox_policy_tempfile(&firefox_policy)?;
    let script = format!(
        "mkdir -p {CA_CERT_DIR} /etc/firefox/policies && rm -f {CA_CERT_DIR}/{CA_CERT_PREFIX}-*.crt && cp {} {} && install -m 0644 {} {} && update-ca-certificates",
        shell_quote_path(cert_path),
        shell_quote_str(&ca_file),
        shell_quote_path(&temp_policy_path),
        shell_quote_str(FIREFOX_POLICIES_PATH),
    );
    let status = Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .status()
        .wrap_err("Failed to install root cert")?;
    let _ = fs::remove_file(&temp_policy_path);
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
    let ca_file = format!("{CA_CERT_DIR}/{CA_CERT_PREFIX}-{fingerprint}.crt");
    if !Path::new(&ca_file).exists() {
        return Ok(false);
    }

    linux_firefox_policy_has_neomist_cert(existing_linux_firefox_policy()?.as_deref(), &ca_file)
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
    let existing_policy = existing_linux_firefox_policy()?;
    let updated_policy = render_linux_firefox_policy_without_neomist(existing_policy.as_deref())?;
    let temp_policy_path = if let Some(policy) = updated_policy.as_deref() {
        Some(write_linux_firefox_policy_tempfile(policy)?)
    } else {
        None
    };
    let policy_command = if let Some(path) = temp_policy_path.as_ref() {
        format!(
            "install -m 0644 {} {}",
            shell_quote_path(path),
            shell_quote_str(FIREFOX_POLICIES_PATH)
        )
    } else {
        format!("rm -f {}", shell_quote_str(FIREFOX_POLICIES_PATH))
    };
    let script = format!(
        "rm -f {CA_CERT_DIR}/{CA_CERT_PREFIX}-*.crt && {policy_command} && update-ca-certificates"
    );
    let status = Command::new("pkexec")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .status()
        .wrap_err("Failed to remove CA certificates")?;
    if let Some(path) = temp_policy_path {
        let _ = fs::remove_file(path);
    }

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

fn cert_schema_matches(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    Ok(fs::read_to_string(path)
        .wrap_err("Failed to read cert schema version")?
        .trim()
        == CERT_SCHEMA_VERSION)
}

fn fresh_serial_number(parts: &[&str]) -> u64 {
    let counter = SERIAL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut hasher = Sha1::new();
    hasher.update(now.to_le_bytes());
    hasher.update(counter.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }

    let digest = hasher.finalize();
    let mut serial_bytes = [0u8; 8];
    serial_bytes.copy_from_slice(&digest[..8]);
    let serial = u64::from_be_bytes(serial_bytes) >> 1;
    serial.max(1)
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

#[cfg(test)]
mod tests {
    use super::{
        FIREFOX_POLICY_DEVICE_NAME, linux_firefox_policy_has_neomist_cert,
        render_linux_firefox_policy_with_neomist, render_linux_firefox_policy_without_neomist,
    };

    #[test]
    fn firefox_policy_install_preserves_existing_entries_and_replaces_neomist_cert() {
        let existing = r#"{
  "policies": {
    "Certificates": {
      "Install": [
        "/usr/local/share/ca-certificates/neomist-ca-old.crt",
        "/opt/acme/internal-root.pem"
      ]
    },
    "Homepage": {
      "URL": "https://example.com"
    }
  }
}"#;

        let updated = render_linux_firefox_policy_with_neomist(
            Some(existing),
            "/usr/local/share/ca-certificates/neomist-ca-new.crt",
        )
        .unwrap();

        assert!(updated.contains("/opt/acme/internal-root.pem"));
        assert!(updated.contains("/usr/local/share/ca-certificates/neomist-ca-new.crt"));
        assert!(!updated.contains("/usr/local/share/ca-certificates/neomist-ca-old.crt"));
        assert!(updated.contains(FIREFOX_POLICY_DEVICE_NAME));
        assert!(updated.contains("https://example.com"));
    }

    #[test]
    fn firefox_policy_uninstall_removes_only_neomist_entries() {
        let existing = r#"{
  "policies": {
    "Certificates": {
      "Install": [
        "/usr/local/share/ca-certificates/neomist-ca-current.crt",
        "/opt/acme/internal-root.pem"
      ]
    },
    "SecurityDevices": {
      "Add": {
        "NeoMist System Trust": "/usr/lib/aarch64-linux-gnu/pkcs11/p11-kit-trust.so",
        "Corp Token": "/usr/lib/libpkcs11.so"
      }
    }
  }
}"#;

        let updated = render_linux_firefox_policy_without_neomist(Some(existing))
            .unwrap()
            .unwrap();

        assert!(!updated.contains("neomist-ca-current.crt"));
        assert!(!updated.contains(FIREFOX_POLICY_DEVICE_NAME));
        assert!(updated.contains("/opt/acme/internal-root.pem"));
        assert!(updated.contains("Corp Token"));
    }

    #[test]
    fn firefox_policy_detection_requires_current_neomist_cert() {
        let existing = r#"{
  "policies": {
    "Certificates": {
      "Install": [
        "/usr/local/share/ca-certificates/neomist-ca-current.crt"
      ]
    }
  }
}"#;

        assert!(linux_firefox_policy_has_neomist_cert(
            Some(existing),
            "/usr/local/share/ca-certificates/neomist-ca-current.crt"
        )
        .unwrap());
        assert!(!linux_firefox_policy_has_neomist_cert(
            Some(existing),
            "/usr/local/share/ca-certificates/neomist-ca-other.crt"
        )
        .unwrap());
    }
}

fn existing_linux_firefox_policy() -> Result<Option<String>> {
    match fs::read_to_string(FIREFOX_POLICIES_PATH) {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).wrap_err("Failed to read Firefox policies"),
    }
}

fn linux_firefox_policy_has_neomist_cert(existing: Option<&str>, ca_file: &str) -> Result<bool> {
    let Some(existing) = existing else {
        return Ok(false);
    };
    let value = parse_linux_firefox_policy(existing)?;
    let Some(policies) = value.get("policies").and_then(Value::as_object) else {
        return Ok(false);
    };
    let Some(certificates) = policies.get("Certificates").and_then(Value::as_object) else {
        return Ok(false);
    };
    let Some(install) = certificates.get("Install").and_then(Value::as_array) else {
        return Ok(false);
    };

    Ok(install.iter().any(|entry| entry.as_str() == Some(ca_file)))
}

fn render_linux_firefox_policy_with_neomist(existing: Option<&str>, ca_file: &str) -> Result<String> {
    let mut value = match existing {
        Some(existing) => parse_linux_firefox_policy(existing)?,
        None => Value::Object(Map::new()),
    };
    let root = value
        .as_object_mut()
        .ok_or_else(|| eyre::eyre!("Firefox policies must be a JSON object"))?;
    let policies = root
        .entry("policies".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| eyre::eyre!("Firefox policies.policies must be a JSON object"))?;
    let certificates = policies
        .entry("Certificates".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| eyre::eyre!("Firefox Certificates policy must be a JSON object"))?;
    let install = certificates
        .entry("Install".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| eyre::eyre!("Firefox Certificates.Install policy must be an array"))?;

    install.retain(|entry| {
        !entry
            .as_str()
            .is_some_and(is_neomist_firefox_certificate_policy_entry)
    });
    install.push(Value::String(ca_file.to_string()));

    if let Some(p11_kit_path) = linux_p11_kit_trust_path() {
        let security_devices = policies
            .entry("SecurityDevices".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| eyre::eyre!("Firefox SecurityDevices policy must be a JSON object"))?;
        let add = security_devices
            .entry("Add".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| eyre::eyre!("Firefox SecurityDevices.Add policy must be a JSON object"))?;
        add.insert(
            FIREFOX_POLICY_DEVICE_NAME.to_string(),
            Value::String(p11_kit_path.to_string()),
        );
    }

    serde_json::to_string_pretty(&value).wrap_err("Failed to serialize Firefox policies")
}

fn render_linux_firefox_policy_without_neomist(existing: Option<&str>) -> Result<Option<String>> {
    let Some(existing) = existing else {
        return Ok(None);
    };
    let mut value = parse_linux_firefox_policy(existing)?;
    let Some(root) = value.as_object_mut() else {
        return Err(eyre::eyre!("Firefox policies must be a JSON object"));
    };
    let Some(policies) = root.get_mut("policies").and_then(Value::as_object_mut) else {
        return Ok(Some(serde_json::to_string_pretty(&value).wrap_err("Failed to serialize Firefox policies")?));
    };

    if let Some(certificates) = policies.get_mut("Certificates").and_then(Value::as_object_mut) {
        if let Some(install) = certificates.get_mut("Install").and_then(Value::as_array_mut) {
            install.retain(|entry| {
                !entry
                    .as_str()
                    .is_some_and(is_neomist_firefox_certificate_policy_entry)
            });
            if install.is_empty() {
                certificates.remove("Install");
            }
        }
        if certificates.is_empty() {
            policies.remove("Certificates");
        }
    }

    if let Some(security_devices) = policies
        .get_mut("SecurityDevices")
        .and_then(Value::as_object_mut)
    {
        if let Some(add) = security_devices.get_mut("Add").and_then(Value::as_object_mut) {
            add.remove(FIREFOX_POLICY_DEVICE_NAME);
            if add.is_empty() {
                security_devices.remove("Add");
            }
        }
        if security_devices.is_empty() {
            policies.remove("SecurityDevices");
        }
    }

    if policies.is_empty() {
        root.remove("policies");
    }

    if root.is_empty() {
        return Ok(None);
    }

    serde_json::to_string_pretty(&value)
        .map(Some)
        .wrap_err("Failed to serialize Firefox policies")
}

fn parse_linux_firefox_policy(existing: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(existing).wrap_err("Failed to parse Firefox policies")?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(eyre::eyre!("Firefox policies must be a JSON object"))
    }
}

fn is_neomist_firefox_certificate_policy_entry(path: &str) -> bool {
    path.starts_with(&format!("{CA_CERT_DIR}/{CA_CERT_PREFIX}-")) && path.ends_with(".crt")
}

fn linux_p11_kit_trust_path() -> Option<&'static str> {
    [
        "/usr/lib/aarch64-linux-gnu/pkcs11/p11-kit-trust.so",
        "/usr/lib/x86_64-linux-gnu/pkcs11/p11-kit-trust.so",
        "/usr/lib/pkcs11/p11-kit-trust.so",
    ]
    .into_iter()
    .find(|path| Path::new(path).exists())
}

fn write_linux_firefox_policy_tempfile(contents: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "neomist-firefox-policies-{}-{}.json",
        std::process::id(),
        fresh_serial_number(&[FIREFOX_POLICIES_PATH])
    ));
    fs::write(&path, contents).wrap_err("Failed to write temporary Firefox policy file")?;
    Ok(path)
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote_str(&path.to_string_lossy())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
