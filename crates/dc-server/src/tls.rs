//! HTTPS on the practice LAN (docs/DECISIONS.md E29, docs/SECURITY.md
//! "Certificate options").
//!
//! Two certificate sources:
//!
//! - **Files.** `DC_TLS_CERT` and `DC_TLS_KEY` name PEM files, for a
//!   certificate from the practice's IT or internal CA (option 1).
//! - **Local CA.** `DC_TLS=local-ca` makes the server act as its own small
//!   certificate authority (option 2). On first start it creates a CA key and
//!   certificate; the key is kept in the secret store (DPAPI on Windows) and
//!   the certificate is written to `<data_dir>/tls/dentalclarity-ca.crt` for
//!   staff to install on each workstation and tablet. A fresh server
//!   certificate, signed by that CA, is made on every start, so it never
//!   expires while the service runs normally. Its names are `localhost`,
//!   the loopback addresses, the bind address when it is not a wildcard,
//!   the host of `DC_PUBLIC_ORIGIN`, and every name in `DC_TLS_HOSTNAMES`.
//!
//! rustls serves TLS 1.2 and 1.3 only.

use crate::secrets::{SecretStore, names};
use anyhow::{Context, Result, bail};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

/// Where the server certificate comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsSource {
    Files { cert: PathBuf, key: PathBuf },
    LocalCa { hostnames: Vec<String> },
}

/// A certificate chain and its private key, PEM. The key is never logged.
pub struct TlsMaterial {
    pub chain_pem: String,
    pub key_pem: String,
}

/// Name of the exported CA certificate under `<data_dir>/tls`.
pub const CA_FILE: &str = "dentalclarity-ca.crt";
/// Validity of the local CA.
const CA_DAYS: i64 = 3650;
/// Validity of a server certificate. Kept under the 398 days browsers accept
/// for public certificates, although a private CA is not held to it.
const SERVER_DAYS: i64 = 397;
const CA_COMMON_NAME: &str = "DentalClarity local CA";

/// The CA certificate path for `data_dir`.
pub fn ca_path(data_dir: &Path) -> PathBuf {
    data_dir.join("tls").join(CA_FILE)
}

/// Loads or makes the certificate for `source`.
pub fn load(
    source: &TlsSource,
    data_dir: &Path,
    secrets: &SecretStore,
    bind: SocketAddr,
    public_origin: &str,
) -> Result<TlsMaterial> {
    match source {
        TlsSource::Files { cert, key } => Ok(TlsMaterial {
            chain_pem: std::fs::read_to_string(cert)
                .with_context(|| format!("reading DC_TLS_CERT {}", cert.display()))?,
            key_pem: std::fs::read_to_string(key)
                .with_context(|| format!("reading DC_TLS_KEY {}", key.display()))?,
        }),
        TlsSource::LocalCa { hostnames } => {
            let names = server_names(hostnames, bind, public_origin);
            local_ca(data_dir, secrets, &names)
        }
    }
}

/// The names a generated server certificate covers.
pub fn server_names(extra: &[String], bind: SocketAddr, public_origin: &str) -> Vec<String> {
    let mut names = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    if !bind.ip().is_unspecified() {
        names.push(bind.ip().to_string());
    }
    if let Some(host) = origin_host(public_origin) {
        names.push(host);
    }
    names.extend(extra.iter().map(|n| n.trim().to_ascii_lowercase()));
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| !n.is_empty() && seen.insert(n.clone()));
    names
}

/// The host of an `https://host:port` origin, without brackets or port.
fn origin_host(origin: &str) -> Option<String> {
    let rest = origin.split_once("://")?.1;
    let authority = rest.split('/').next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn ca_params() -> Result<CertificateParams> {
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, CA_COMMON_NAME);
    dn.push(DnType::OrganizationName, "DentalClarity");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    Ok(params)
}

/// Loads the local CA, creating it on first use, and signs a server
/// certificate for `names`.
fn local_ca(data_dir: &Path, secrets: &SecretStore, names: &[String]) -> Result<TlsMaterial> {
    let ca_file = ca_path(data_dir);
    let now = OffsetDateTime::now_utc();
    let ca_key = match secrets.get_string(names::TLS_CA_KEY)? {
        Some(pem) if ca_file.exists() => {
            KeyPair::from_pem(&pem).context("reading the stored local CA key")?
        }
        _ => {
            let key = KeyPair::generate()?;
            let mut params = ca_params()?;
            params.not_before = now - Duration::days(1);
            params.not_after = now + Duration::days(CA_DAYS);
            let cert = params.self_signed(&key)?;
            std::fs::create_dir_all(ca_file.parent().expect("tls dir"))?;
            secrets.set(names::TLS_CA_KEY, key.serialize_pem().as_bytes())?;
            std::fs::write(&ca_file, cert.pem())
                .with_context(|| format!("writing {}", ca_file.display()))?;
            tracing::warn!(
                path = %ca_file.display(),
                "created a new local certificate authority; install this certificate on every workstation and tablet"
            );
            key
        }
    };
    let ca_pem = std::fs::read_to_string(&ca_file)
        .with_context(|| format!("reading {}", ca_file.display()))?;
    let issuer = Issuer::new(ca_params()?, ca_key);

    if names.is_empty() {
        bail!("the server certificate needs at least one name");
    }
    let mut params = CertificateParams::new(names.to_vec())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, names[0].as_str());
    params.distinguished_name = dn;
    params.not_before = now - Duration::hours(1);
    params.not_after = now + Duration::days(SERVER_DAYS);
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key = KeyPair::generate()?;
    let cert = params.signed_by(&key, &issuer)?;
    tracing::info!(names = ?names, "issued a server certificate from the local CA");
    Ok(TlsMaterial {
        chain_pem: format!("{}{}", cert.pem(), ca_pem),
        key_pem: key.serialize_pem(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cover_the_bind_address_origin_and_extras() {
        let names = server_names(
            &["Front-Desk.practice.lan".into(), "localhost".into()],
            "192.168.1.20:8443".parse().unwrap(),
            "https://dentalclarity.practice.lan:8443",
        );
        assert_eq!(
            names,
            [
                "localhost",
                "127.0.0.1",
                "::1",
                "192.168.1.20",
                "dentalclarity.practice.lan",
                "front-desk.practice.lan"
            ]
        );
        let names = server_names(
            &[],
            "0.0.0.0:8443".parse().unwrap(),
            "https://[fd00::5]:8443",
        );
        assert_eq!(names, ["localhost", "127.0.0.1", "::1", "fd00::5"]);
    }

    #[test]
    fn the_local_ca_is_made_once_and_reused() {
        let dir = std::env::temp_dir().join(format!("dc-tls-{}", crate::credentials::new_token()));
        let secrets = SecretStore::new(&dir);
        let names = vec!["localhost".to_owned(), "192.168.1.20".to_owned()];
        let first = local_ca(&dir, &secrets, &names).unwrap();
        let ca = std::fs::read_to_string(ca_path(&dir)).unwrap();
        assert!(first.chain_pem.ends_with(&ca));
        assert!(first.key_pem.contains("PRIVATE KEY"));
        let second = local_ca(&dir, &secrets, &names).unwrap();
        assert_eq!(std::fs::read_to_string(ca_path(&dir)).unwrap(), ca);
        assert_ne!(first.key_pem, second.key_pem);
        assert_eq!(second.chain_pem.matches("BEGIN CERTIFICATE").count(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
