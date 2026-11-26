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
/// | `DC_CLIENT_IP_HEADER` | unset (needs `DC_TRUSTED_PROXY`)   |
/// | `DC_HSTS`           | `false`                               |
/// | `DC_DATA_DIR`       | `./data`                              |
/// | `DC_FIXTURE`        | `fixtures/synthetic/practice.toml`    |
/// | `DC_KNOWLEDGE_DIR`  | `knowledge`                           |
/// | `DC_TEXT_MODEL`     | app-managed default (demo only)       |
/// | `DC_DEMO_MONTHLY_BUDGET_USD` | `25`                         |
///
/// `OPENROUTER_API_KEY` is read where the demo provider is built, and is
/// deliberately not part of this `Debug` struct.
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub bind: SocketAddr,
    pub http: HttpConfig,
    pub data_dir: PathBuf,
    pub fixture: PathBuf,
    pub knowledge_dir: PathBuf,
    /// Override of the demo text model id for writer and reviewer.
    pub text_model: Option<String>,
    /// Monthly cap on demo model spend in US dollars.
    pub demo_monthly_budget_usd: f64,
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
                "clinical mode on the network needs HTTPS. Set DC_PUBLIC_ORIGIN to an https:// address served through a TLS proxy, or bind to 127.0.0.1 for use on this machine only."
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
            fixture: var("DC_FIXTURE")
                .unwrap_or_else(|| "fixtures/synthetic/practice.toml".into())
                .into(),
            knowledge_dir: var("DC_KNOWLEDGE_DIR")
                .unwrap_or_else(|| "knowledge".into())
                .into(),
            text_model: var("DC_TEXT_MODEL").map(|m| m.trim().to_owned()),
            demo_monthly_budget_usd,
        })
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
}
