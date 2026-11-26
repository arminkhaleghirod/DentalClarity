use anyhow::{Context, Result};
use dc_core::Mode;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Startup configuration, read from environment variables.
///
/// | Variable            | Default                               |
/// |---------------------|---------------------------------------|
/// | `DC_MODE`           | `demo`                                |
/// | `DC_BIND`           | `127.0.0.1:8080`                      |
/// | `DC_DATA_DIR`       | `./data`                              |
/// | `DC_FIXTURE`        | `fixtures/synthetic/practice.toml`    |
/// | `DC_KNOWLEDGE_DIR`  | `knowledge`                           |
#[derive(Debug, Clone)]
pub struct Config {
    pub mode: Mode,
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub fixture: PathBuf,
    pub knowledge_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let var =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
        let mode = var("DC_MODE", "demo").parse::<Mode>().context("DC_MODE")?;
        let bind = var("DC_BIND", "127.0.0.1:8080")
            .parse()
            .context("DC_BIND")?;
        Ok(Self {
            mode,
            bind,
            data_dir: var("DC_DATA_DIR", "data").into(),
            fixture: var("DC_FIXTURE", "fixtures/synthetic/practice.toml").into(),
            knowledge_dir: var("DC_KNOWLEDGE_DIR", "knowledge").into(),
        })
    }
}
