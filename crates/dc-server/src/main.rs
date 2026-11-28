//! `dentalclarity` command line.
//!
//! ```text
//! dentalclarity                     run the server in this console
//! dentalclarity service install     register the Windows service (Administrator)
//! dentalclarity service uninstall   stop and remove the Windows service
//! dentalclarity service run         started by Windows, not by hand
//! dentalclarity secret set NAME     store a secret read from standard input
//! dentalclarity secret delete NAME  remove a stored secret
//! dentalclarity tls ca-path         print where the local CA certificate is
//! ```

use anyhow::{Context, Result, bail};
use dc_server::config::Config;
use dc_server::secrets::SecretStore;
use dc_server::server::{self, RunOptions};
use dc_server::{service, tls};
use std::io::BufRead;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

const USAGE: &str =
    "usage: dentalclarity [service install|uninstall|run | secret set|delete NAME | tls ca-path]";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] => run_console(),
        ["service", "install"] => service::install(),
        ["service", "uninstall"] => service::uninstall(),
        ["service", "run"] => service::run(),
        ["secret", "set", name] => {
            load_settings();
            let store = SecretStore::new(&data_dir());
            eprintln!("Enter the value for {name} and press Enter:");
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            let value = line.trim_end_matches(['\r', '\n']);
            if value.trim().is_empty() {
                bail!("no value given; nothing was stored");
            }
            store.set(name, value.as_bytes())?;
            let how = if dc_server::secrets::PROTECTED {
                "protected with DPAPI"
            } else {
                "not encrypted on this system; file permissions only"
            };
            println!("Stored {name} ({how}). Restart DentalClarity to use it.");
            Ok(())
        }
        ["secret", "delete", name] => {
            load_settings();
            let removed = SecretStore::new(&data_dir()).delete(name)?;
            println!(
                "{}",
                if removed {
                    "Removed."
                } else {
                    "Nothing was stored under that name."
                }
            );
            Ok(())
        }
        ["tls", "ca-path"] => {
            load_settings();
            println!("{}", tls::ca_path(&data_dir()).display());
            Ok(())
        }
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

/// Loads `.env` from the working directory, or on Windows the service's
/// settings file when there is no `.env`, so commands run by an
/// administrator see the same data directory as the service. Values already
/// set in the environment win.
fn load_settings() -> Option<PathBuf> {
    if let Ok(path) = dotenvy::dotenv() {
        return Some(path);
    }
    if cfg!(windows) {
        let path = service::program_data().join(service::SETTINGS_FILE);
        if dotenvy::from_path(&path).is_ok() {
            if std::env::var_os("DC_DATA_DIR").is_none() {
                // SAFETY: single-threaded at this point; no runtime yet.
                unsafe { std::env::set_var("DC_DATA_DIR", service::program_data().join("data")) };
            }
            return Some(path);
        }
    }
    None
}

fn data_dir() -> PathBuf {
    std::env::var("DC_DATA_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "data".into())
        .into()
}

fn run_console() -> Result<()> {
    let loaded = load_settings();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    if let Some(path) = &loaded {
        tracing::info!(path = %path.display(), "loaded settings file");
    }
    let config = Config::from_env()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?
        .block_on(server::run(config, RunOptions::default(), async {
            let _ = tokio::signal::ctrl_c().await;
        }))
}
