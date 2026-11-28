use anyhow::{Context, Result, bail};
use dc_core::Mode;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use crate::tls::TlsSource;

/// Startup configuration, read from environment variables.
///
/// | Variable            | Default                               |
/// |---------------------|---------------------------------------|
/// | `DC_MODE`           | `demo`                                |
/// | `DC_BIND`           | `127.0.0.1:8080`                      |
/// | `DC_PUBLIC_ORIGIN`  | `http://` + `DC_BIND`                 |
/// | `DC_TRUSTED_PROXY`  | unset                                 |
/// | `DC_CLIENT_IP_HEADER` | unset (needs `DC_TRUSTED_PROXY`)   |
/// | `DC_HSTS`           | `false`                               |
/// | `DC_DATA_DIR`       | `./data`                              |
/// | `DC_PMS`            | `synthetic`                           |
/// | `DC_FIXTURE`        | `fixtures/synthetic/practice.toml`    |
/// | `DC_PRACTICE_NAME`  | the fixture's name, or `Your practice` |
/// | `DC_OPENDENTAL_*`   | see [`OpenDentalConfig`]              |
/// | `DC_KNOWLEDGE_DIR`  | `knowledge`                           |
/// | `DC_TEXT_MODEL`     | app-managed default (demo only)       |
/// | `DC_DEMO_MONTHLY_BUDGET_USD` | `25`                         |
/// | `DC_TLS`            | unset; `local-ca` for a generated CA  |
/// | `DC_TLS_CERT`, `DC_TLS_KEY` | unset; PEM files              |
/// | `DC_TLS_HOSTNAMES`  | unset; extra certificate names        |
/// | `DC_AI_PROVIDER`    | unset; `aws` in clinical mode         |
///
/// With TLS on, `DC_PUBLIC_ORIGIN` defaults to `https://` + `DC_BIND`.
/// The AWS provider variables are read in [`crate::clinical`].
///
/// `OPENROUTER_API_KEY` is read where the demo provider is built, and is
/// deliberately not part of this `Debug` struct.
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub bind: SocketAddr,
    pub http: HttpConfig,
    pub data_dir: PathBuf,
    pub pms: PmsConfig,
    pub fixture: PathBuf,
    /// Shown until a practice name is saved in settings.
    pub practice_name: Option<String>,
    pub knowledge_dir: PathBuf,
    /// Override of the demo text model id for writer and reviewer.
    pub text_model: Option<String>,
    /// Monthly cap on demo model spend in US dollars.
    pub demo_monthly_budget_usd: f64,
    /// HTTPS served by this process. `None` serves plain HTTP, for a TLS
    /// proxy in front, the demo platform, or loopback use.
    pub tls: Option<TlsSource>,
}

/// Settings the request-handling code needs.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Scheme, host and port browsers use to reach the app, e.g.
    /// `https://demo.example.org`. Used for the Origin check on writes.
    pub public_origin: String,
    /// Reverse proxies whose forwarding headers are trusted for client IPs.
    /// One address or a comma-separated list of CIDR ranges.
    pub trusted_proxy: Option<ProxyRanges>,
    /// Header the trusted proxy sets to the client address, e.g.
    /// `fly-client-ip`. Read only from trusted proxies. Without it, the
    /// rightmost `X-Forwarded-For` entry is used.
    pub client_ip_header: Option<String>,
    /// Send `Strict-Transport-Security`. Enable only with a certificate every
    /// client trusts; with a self-signed certificate it locks browsers out.
    pub hsts: bool,
}

impl HttpConfig {
    /// Cookies get the `Secure` flag and `__Host-` prefix only over HTTPS.
    /// Plain HTTP is for local development on this machine.
    pub fn secure(&self) -> bool {
        self.public_origin.starts_with("https://")
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let mode = var("DC_MODE")
            .unwrap_or_else(|| "demo".into())
            .parse::<Mode>()
            .context("DC_MODE")?;
        let bind: SocketAddr = var("DC_BIND")
            .unwrap_or_else(|| "127.0.0.1:8080".into())
            .parse()
            .context("DC_BIND")?;
        let tls = tls_source(&var)?;
        let scheme = if tls.is_some() { "https" } else { "http" };
        let public_origin = var("DC_PUBLIC_ORIGIN")
            .unwrap_or_else(|| format!("{scheme}://{bind}"))
            .trim_end_matches('/')
            .to_owned();
        if !public_origin.starts_with("http://") && !public_origin.starts_with("https://") {
            bail!("DC_PUBLIC_ORIGIN must start with http:// or https://");
        }
        if tls.is_some() && !public_origin.starts_with("https://") {
            bail!("the server speaks HTTPS, so DC_PUBLIC_ORIGIN must start with https://");
        }
        let trusted_proxy = var("DC_TRUSTED_PROXY")
            .map(|v| v.parse())
            .transpose()
            .context("DC_TRUSTED_PROXY")?;
        let client_ip_header = var("DC_CLIENT_IP_HEADER").map(|h| h.trim().to_ascii_lowercase());
        if client_ip_header.is_some() && trusted_proxy.is_none() {
            bail!("DC_CLIENT_IP_HEADER needs DC_TRUSTED_PROXY, or any client could forge it");
        }
        let hsts = matches!(var("DC_HSTS").as_deref(), Some("1" | "true" | "yes"));
        if mode == Mode::Clinical
            && !public_origin.starts_with("https://")
            && !bind.ip().is_loopback()
        {
            bail!(
                "clinical mode on the network needs HTTPS. Set DC_TLS=local-ca or DC_TLS_CERT and DC_TLS_KEY, or set DC_PUBLIC_ORIGIN to an https:// address served through a TLS proxy, or bind to 127.0.0.1 for use on this machine only."
            );
        }
        let demo_monthly_budget_usd = match var("DC_DEMO_MONTHLY_BUDGET_USD") {
            Some(v) => {
                let value: f64 = v.trim().parse().context("DC_DEMO_MONTHLY_BUDGET_USD")?;
                if !value.is_finite() || value < 0.0 {
                    bail!("DC_DEMO_MONTHLY_BUDGET_USD must be a positive number");
                }
                value
            }
            None => 25.0,
        };
        Ok(Self {
            mode,
            bind,
            http: HttpConfig {
                public_origin,
                trusted_proxy,
                client_ip_header,
                hsts,
            },
            data_dir: var("DC_DATA_DIR").unwrap_or_else(|| "data".into()).into(),
            pms: PmsConfig::from_vars(mode, var)?,
            fixture: var("DC_FIXTURE")
                .unwrap_or_else(|| "fixtures/synthetic/practice.toml".into())
                .into(),
            practice_name: var("DC_PRACTICE_NAME").map(|n| n.trim().to_owned()),
            knowledge_dir: var("DC_KNOWLEDGE_DIR")
                .unwrap_or_else(|| "knowledge".into())
                .into(),
            text_model: var("DC_TEXT_MODEL").map(|m| m.trim().to_owned()),
            demo_monthly_budget_usd,
            tls,
        })
    }
}

/// Which practice management system the server reads (DECISIONS D14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PmsConfig {
    /// The fictional practice in `DC_FIXTURE`.
    Synthetic,
    /// An Open Dental database, clinical mode only.
    OpenDental(OpenDentalConfig),
}

/// Connection to the Open Dental database over a read-only MySQL user.
///
/// | Variable                 | Default                                 |
/// |--------------------------|-----------------------------------------|
/// | `DC_OPENDENTAL_HOST`     | `127.0.0.1`                             |
/// | `DC_OPENDENTAL_PORT`     | `3306`                                  |
/// | `DC_OPENDENTAL_DATABASE` | `opendental`                            |
/// | `DC_OPENDENTAL_USER`     | required                                |
/// | `DC_OPENDENTAL_PASSWORD` | required                                |
/// | `DC_OPENDENTAL_TLS`      | `preferred` on this machine, else `verify-identity` |
/// | `DC_OPENDENTAL_TLS_CA`   | unset (the operating system's trust store) |
#[derive(Clone, PartialEq, Eq)]
pub struct OpenDentalConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    pub tls: DbTls,
    pub tls_ca: Option<PathBuf>,
}

impl std::fmt::Debug for OpenDentalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenDentalConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &"[redacted]")
            .field("tls", &self.tls)
            .field("tls_ca", &self.tls_ca)
            .finish()
    }
}

/// TLS for the database connection, with MySQL's names for the modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbTls {
    Disabled,
    Preferred,
    Required,
    VerifyCa,
    VerifyIdentity,
}

impl std::str::FromStr for DbTls {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(
            match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
                "disabled" => DbTls::Disabled,
                "preferred" => DbTls::Preferred,
                "required" => DbTls::Required,
                "verify-ca" => DbTls::VerifyCa,
                "verify-identity" => DbTls::VerifyIdentity,
                other => bail!(
                    "unknown TLS mode {other:?}; use disabled, preferred, required, verify-ca or verify-identity"
                ),
            },
        )
    }
}

fn is_local_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl PmsConfig {
    pub fn from_vars(mode: Mode, var: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let choice = var("DC_PMS").unwrap_or_else(|| "synthetic".into());
        match choice.trim().to_ascii_lowercase().as_str() {
            "synthetic" => Ok(PmsConfig::Synthetic),
            "opendental" => {
                if mode == Mode::Demo {
                    bail!(
                        "demo mode reads the synthetic practice only; DC_PMS=opendental needs DC_MODE=clinical"
                    );
                }
                let host = var("DC_OPENDENTAL_HOST")
                    .unwrap_or_else(|| "127.0.0.1".into())
                    .trim()
                    .to_owned();
                let local = is_local_host(&host);
                let tls = match var("DC_OPENDENTAL_TLS") {
                    Some(v) => v.parse().context("DC_OPENDENTAL_TLS")?,
                    None if local => DbTls::Preferred,
                    None => DbTls::VerifyIdentity,
                };
                if !local && matches!(tls, DbTls::Disabled | DbTls::Preferred) {
                    bail!(
                        "the Open Dental database at {host} is on another machine, so DC_OPENDENTAL_TLS must be required, verify-ca or verify-identity"
                    );
                }
                Ok(PmsConfig::OpenDental(OpenDentalConfig {
                    port: var("DC_OPENDENTAL_PORT")
                        .map(|p| p.trim().parse())
                        .transpose()
                        .context("DC_OPENDENTAL_PORT")?
                        .unwrap_or(3306),
                    host,
                    database: var("DC_OPENDENTAL_DATABASE")
                        .unwrap_or_else(|| "opendental".into())
                        .trim()
                        .to_owned(),
                    user: var("DC_OPENDENTAL_USER")
                        .context("DC_OPENDENTAL_USER is required with DC_PMS=opendental")?
                        .trim()
                        .to_owned(),
                    password: var("DC_OPENDENTAL_PASSWORD")
                        .context("DC_OPENDENTAL_PASSWORD is required with DC_PMS=opendental")?,
                    tls,
                    tls_ca: var("DC_OPENDENTAL_TLS_CA").map(PathBuf::from),
                }))
            }
            other => bail!("DC_PMS must be synthetic or opendental, got {other:?}"),
        }
    }
}

/// Reads the TLS settings: certificate files, the generated local CA, or none.
fn tls_source(var: &dyn Fn(&str) -> Option<String>) -> Result<Option<TlsSource>> {
    let files = match (var("DC_TLS_CERT"), var("DC_TLS_KEY")) {
        (Some(cert), Some(key)) => Some(TlsSource::Files {
            cert: cert.trim().into(),
            key: key.trim().into(),
        }),
        (None, None) => None,
        _ => bail!("set both DC_TLS_CERT and DC_TLS_KEY, or neither"),
    };
    let mode = var("DC_TLS").map(|v| v.trim().to_ascii_lowercase());
    match (mode.as_deref(), files) {
        (Some("local-ca"), Some(_)) => {
            bail!("DC_TLS=local-ca and DC_TLS_CERT cannot be used together")
        }
        (Some("local-ca"), None) => Ok(Some(TlsSource::LocalCa {
            hostnames: var("DC_TLS_HOSTNAMES")
                .map(|v| {
                    v.split(',')
                        .map(|n| n.trim().to_owned())
                        .filter(|n| !n.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        })),
        (Some("off"), Some(_)) => bail!("DC_TLS=off conflicts with DC_TLS_CERT"),
        (None | Some("off"), None) => Ok(None),
        (None | Some("files"), files @ Some(_)) => Ok(files),
        (Some("files"), None) => bail!("DC_TLS=files needs DC_TLS_CERT and DC_TLS_KEY"),
        (Some(other), _) => bail!("DC_TLS must be local-ca, files or off, not {other:?}"),
    }
}

/// A set of address ranges, parsed from `"10.0.0.1"` or `"fdaa::/16, 172.16.0.0/12"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyRanges(Vec<(IpAddr, u8)>);

impl ProxyRanges {
    pub fn contains(&self, ip: IpAddr) -> bool {
        self.0
            .iter()
            .any(|(net, prefix)| in_range(*net, *prefix, ip))
    }
}

fn in_range(net: IpAddr, prefix: u8, ip: IpAddr) -> bool {
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(a)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix))
            };
            u32::from(n) & mask == u32::from(a) & mask
        }
        (IpAddr::V6(n), IpAddr::V6(a)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix))
            };
            u128::from(n) & mask == u128::from(a) & mask
        }
        // An IPv4 peer seen as an IPv4-mapped IPv6 address.
        (IpAddr::V4(_), IpAddr::V6(a)) => a
            .to_ipv4_mapped()
            .is_some_and(|v4| in_range(net, prefix, IpAddr::V4(v4))),
        _ => false,
    }
}

impl std::str::FromStr for ProxyRanges {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let mut ranges = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (addr, prefix) = match part.split_once('/') {
                Some((a, p)) => (a.parse::<IpAddr>()?, p.parse::<u8>()?),
                None => {
                    let a = part.parse::<IpAddr>()?;
                    (a, if a.is_ipv4() { 32 } else { 128 })
                }
            };
            let max = if addr.is_ipv4() { 32 } else { 128 };
            if prefix > max {
                bail!("prefix /{prefix} is too long for {addr}");
            }
            ranges.push((addr, prefix));
        }
        if ranges.is_empty() {
            bail!("no address or range given");
        }
        Ok(ProxyRanges(ranges))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_addresses_and_ranges() {
        let r: ProxyRanges = "10.0.0.1".parse().unwrap();
        assert!(r.contains("10.0.0.1".parse().unwrap()));
        assert!(!r.contains("10.0.0.2".parse().unwrap()));
        let r: ProxyRanges = "fdaa::/16, 172.16.0.0/12".parse().unwrap();
        assert!(r.contains("fdaa:0:1::2".parse().unwrap()));
        assert!(r.contains("172.20.1.9".parse().unwrap()));
        assert!(!r.contains("fdab::1".parse().unwrap()));
        assert!(!r.contains("8.8.8.8".parse().unwrap()));
        assert!("10.0.0.1/40".parse::<ProxyRanges>().is_err());
        assert!("".parse::<ProxyRanges>().is_err());
    }

    fn pms(mode: Mode, vars: &[(&str, &str)]) -> Result<PmsConfig> {
        let vars: std::collections::HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        PmsConfig::from_vars(mode, |name| vars.get(name).cloned())
    }

    const CREDENTIALS: [(&str, &str); 3] = [
        ("DC_PMS", "opendental"),
        ("DC_OPENDENTAL_USER", "dentalclarity_ro"),
        ("DC_OPENDENTAL_PASSWORD", "example-password"),
    ];

    #[test]
    fn synthetic_is_the_default_connector() {
        assert_eq!(pms(Mode::Demo, &[]).unwrap(), PmsConfig::Synthetic);
        assert_eq!(pms(Mode::Clinical, &[]).unwrap(), PmsConfig::Synthetic);
        assert!(pms(Mode::Clinical, &[("DC_PMS", "dentrix")]).is_err());
    }

    #[test]
    fn demo_mode_refuses_open_dental() {
        let err = pms(Mode::Demo, &CREDENTIALS).unwrap_err();
        assert!(err.to_string().contains("demo mode"), "{err}");
    }

    #[test]
    fn open_dental_needs_credentials() {
        assert!(pms(Mode::Clinical, &CREDENTIALS[..2]).is_err());
        assert!(pms(Mode::Clinical, &[CREDENTIALS[0], CREDENTIALS[2]]).is_err());
        let PmsConfig::OpenDental(od) = pms(Mode::Clinical, &CREDENTIALS).unwrap() else {
            panic!("expected Open Dental");
        };
        assert_eq!(
            (od.host.as_str(), od.port, od.database.as_str()),
            ("127.0.0.1", 3306, "opendental")
        );
        assert_eq!(od.tls, DbTls::Preferred);
        assert!(!format!("{od:?}").contains("example-password"));
    }

    #[test]
    fn remote_databases_need_tls() {
        let remote = |tls: Option<&str>| {
            let mut vars = CREDENTIALS.to_vec();
            vars.push(("DC_OPENDENTAL_HOST", "od-server.practice.lan"));
            if let Some(tls) = tls {
                vars.push(("DC_OPENDENTAL_TLS", tls));
            }
            pms(Mode::Clinical, &vars)
        };
        let Ok(PmsConfig::OpenDental(od)) = remote(None) else {
            panic!("expected Open Dental");
        };
        assert_eq!(od.tls, DbTls::VerifyIdentity);
        assert!(remote(Some("required")).is_ok());
        assert!(remote(Some("verify_ca")).is_ok());
        assert!(remote(Some("preferred")).is_err());
        assert!(remote(Some("disabled")).is_err());
        assert!(remote(Some("sometimes")).is_err());
    }

    #[test]
    fn reads_tls_settings() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(tls_source(&env(&[])).unwrap(), None);
        assert_eq!(
            tls_source(&env(&[
                ("DC_TLS", "local-ca"),
                ("DC_TLS_HOSTNAMES", "a.lan, 10.0.0.5,")
            ]))
            .unwrap(),
            Some(TlsSource::LocalCa {
                hostnames: vec!["a.lan".into(), "10.0.0.5".into()]
            })
        );
        assert!(matches!(
            tls_source(&env(&[("DC_TLS_CERT", "c.pem"), ("DC_TLS_KEY", "k.pem")])).unwrap(),
            Some(TlsSource::Files { .. })
        ));
        assert!(tls_source(&env(&[("DC_TLS_CERT", "c.pem")])).is_err());
        assert!(tls_source(&env(&[("DC_TLS", "self-signed")])).is_err());
        assert!(
            tls_source(&env(&[
                ("DC_TLS", "local-ca"),
                ("DC_TLS_CERT", "c.pem"),
                ("DC_TLS_KEY", "k.pem")
            ]))
            .is_err()
        );
    }
}
