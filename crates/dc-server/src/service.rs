//! Windows service: `dentalclarity service install | uninstall | run`
//! (docs/DECISIONS.md E10, E23).
//!
//! `install` registers the service `DentalClarity` to start automatically as
//! the virtual account `NT SERVICE\DentalClarity`, with `service run` as its
//! arguments. `run` is what the service control manager starts: it reads the
//! settings file `%ProgramData%\DentalClarity\dentalclarity.env`, logs to
//! `%ProgramData%\DentalClarity\logs\dentalclarity.log`, and serves until the
//! service is stopped. `uninstall` stops and removes the service and leaves
//! the data in place.

use std::path::PathBuf;

/// Service name, also the virtual account name.
pub const NAME: &str = "DentalClarity";
/// Settings file name under [`program_data`].
pub const SETTINGS_FILE: &str = "dentalclarity.env";

/// `%ProgramData%\DentalClarity`, where the service keeps its settings,
/// data and logs.
pub fn program_data() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join(NAME)
}

#[cfg(windows)]
pub use imp::{install, run, uninstall};

#[cfg(not(windows))]
pub fn install() -> anyhow::Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

#[cfg(not(windows))]
pub fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

#[cfg(not(windows))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

#[cfg(windows)]
mod imp {
    use super::{NAME, SETTINGS_FILE, program_data};
    use crate::config::Config;
    use crate::server::{self, RunOptions};
    use anyhow::{Context, Result};
    use std::ffi::OsString;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    const ACCOUNT: &str = r"NT SERVICE\DentalClarity";

    pub fn install() -> Result<()> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .context("opening the service manager (run as Administrator)")?;
        let info = ServiceInfo {
            name: OsString::from(NAME),
            display_name: OsString::from("DentalClarity"),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: std::env::current_exe()?,
            launch_arguments: vec![OsString::from("service"), OsString::from("run")],
            dependencies: vec![],
            account_name: Some(OsString::from(ACCOUNT)),
            account_password: None,
        };
        let service = manager
            .create_service(&info, ServiceAccess::CHANGE_CONFIG)
            .context("creating the service")?;
        service.set_description(
            "Explains proposed dental treatment to patients. Serves the DentalClarity web app on the practice network.",
        )?;
        println!("Installed the {NAME} service as {ACCOUNT}.");
        println!(
            "Settings: {}. Start it with: sc.exe start {NAME}",
            program_data().join(SETTINGS_FILE).display()
        );
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .context("opening the service manager (run as Administrator)")?;
        let service = manager
            .open_service(
                NAME,
                ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
            )
            .context("opening the service")?;
        // Marked for deletion now; removed once it has stopped.
        service.delete()?;
        if service.query_status()?.current_state != ServiceState::Stopped {
            service.stop()?;
            let deadline = Instant::now() + Duration::from_secs(30);
            while service.query_status()?.current_state != ServiceState::Stopped {
                if Instant::now() > deadline {
                    anyhow::bail!("the service did not stop within 30 s");
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        println!(
            "Removed the {NAME} service. Data in {} is kept.",
            program_data().display()
        );
        Ok(())
    }

    pub fn run() -> Result<()> {
        service_dispatcher::start(NAME, ffi_service_main).context(
            "starting the service dispatcher (this command is run by Windows, not by hand)",
        )
    }

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(err) = run_service() {
            tracing::error!(error = %format!("{err:#}"), "the service stopped with an error");
        }
    }

    fn init_logging() -> Result<()> {
        let dir = program_data().join("logs");
        std::fs::create_dir_all(&dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("dentalclarity.log"))?;
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init();
        Ok(())
    }

    fn run_service() -> Result<()> {
        let base = program_data();
        // Paths in the settings file are relative to the install directory,
        // where the knowledge base and fixtures are.
        if let Some(dir) = std::env::current_exe()?.parent() {
            std::env::set_current_dir(dir)?;
        }
        let settings = base.join(SETTINGS_FILE);
        let loaded = dotenvy::from_path(&settings);
        // SAFETY: called before the runtime or any other thread starts.
        if std::env::var_os("DC_DATA_DIR").is_none() {
            unsafe { std::env::set_var("DC_DATA_DIR", base.join("data")) };
        }
        init_logging()?;
        match loaded {
            Ok(()) => tracing::info!(path = %settings.display(), "loaded settings file"),
            Err(err) => tracing::warn!(path = %settings.display(), %err, "no settings file"),
        }

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let stop_tx = Mutex::new(Some(stop_tx));
        let handler = move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Some(tx) = stop_tx.lock().expect("stop lock").take() {
                    let _ = tx.send(());
                }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status = service_control_handler::register(NAME, handler)?;
        let report = |state, accept, exit| {
            status.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code: ServiceExitCode::Win32(exit),
                checkpoint: 0,
                wait_hint: Duration::from_secs(15),
                process_id: None,
            })
        };
        report(
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            0,
        )?;

        let result = (|| -> Result<()> {
            let config = Config::from_env()?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(server::run(config, RunOptions { service: true }, async {
                let _ = stop_rx.await;
                tracing::info!("service stop requested");
            }))
        })();
        if let Err(err) = &result {
            tracing::error!(error = %format!("{err:#}"), "DentalClarity stopped with an error");
        }
        report(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            u32::from(result.is_err()),
        )?;
        result
    }
}
