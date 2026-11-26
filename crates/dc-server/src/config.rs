use anyhow::{Context, Result, bail};
use dc_core::Mode;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

/// Startup configuration, read from environment variables.
///
/// | Variable            | Default                               |
/// |---------------------|---------------------------------------|
/// | `DC_MODE`           | `demo`                                |
/// | `DC_BIND`           | `127.0.0.1:8080`                      |
/// | `DC_PUBLIC_ORIGIN`  | `http://` + `DC_BIND`                 |
/// | `DC_TRUSTED_PROXY`  | unset                                 |
/// | `DC_HSTS`           | `false`                               |
/// | `DC_DATA_DIR`       | `./data`                              |
/// | `DC_FIXTURE`        | `fixtures/synthetic/practice.toml`    |
/// | `DC_KNOWLEDGE_DIR`  | `knowledge`                           |
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub bind: SocketAddr,
    pub http: HttpConfig,
    pub data_dir: PathBuf,
    pub fixture: PathBuf,
    pub knowledge_dir: PathBuf,
}

/// Settings the request-handling code needs.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// Scheme, host and port browsers use to reach the app, e.g.
    /// `https://demo.example.org`. Used for the Origin check on writes.
    pub public_origin: String,
    /// Reverse proxy whose `X-Forwarded-For` header is trusted for client IPs.
    pub trusted_proxy: Option<IpAddr>,
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
        let public_origin = var("DC_PUBLIC_ORIGIN")
            .unwrap_or_else(|| format!("http://{bind}"))
            .trim_end_matches('/')
            .to_owned();
        if !public_origin.starts_with("http://") && !public_origin.starts_with("https://") {
            bail!("DC_PUBLIC_ORIGIN must start with http:// or https://");
        }
        let trusted_proxy = var("DC_TRUSTED_PROXY")
            .map(|v| v.parse())
            .transpose()
            .context("DC_TRUSTED_PROXY")?;
        let hsts = matches!(var("DC_HSTS").as_deref(), Some("1" | "true" | "yes"));
        if mode == Mode::Clinical
            && !public_origin.starts_with("https://")
            && !bind.ip().is_loopback()
        {
            bail!(
                "clinical mode on the network needs HTTPS. Set DC_PUBLIC_ORIGIN to an https:// address served through a TLS proxy, or bind to 127.0.0.1 for use on this machine only."
            );
        }
        Ok(Self {
            mode,
            bind,
            http: HttpConfig {
                public_origin,
                trusted_proxy,
                hsts,
            },
            data_dir: var("DC_DATA_DIR").unwrap_or_else(|| "data".into()).into(),
            fixture: var("DC_FIXTURE")
                .unwrap_or_else(|| "fixtures/synthetic/practice.toml".into())
                .into(),
            knowledge_dir: var("DC_KNOWLEDGE_DIR")
                .unwrap_or_else(|| "knowledge".into())
                .into(),
        })
    }
}
