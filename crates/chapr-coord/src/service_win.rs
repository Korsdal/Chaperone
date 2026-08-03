//! Native Windows Service (SCM) integration (E-016).
//!
//! `chapr-coord run-service` (the binPath the installed service uses) hands the
//! process to the SCM dispatcher; the generated service main brings up the
//! server on a tokio runtime and stops it when the SCM asks. `install`/`start`
//! are used by the setup wizard to register and launch the service. Requires an
//! elevated (admin) context to install.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

const SERVICE_NAME: &str = "chapr-coord";
const SERVICE_DISPLAY: &str = "Chaperone coordination service";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
/// The config path is handed to the SCM-launched service main via this env var
/// (set by `run` before dispatch, in the same process).
const CONFIG_ENV: &str = "CHAPR_COORD_SERVICE_CONFIG";

/// Entry for `chapr-coord run-service`: hand control to the SCM dispatcher.
pub fn run(config: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(p) = &config {
        std::env::set_var(CONFIG_ENV, p);
    }
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(error = %e, "chapr-coord service failed");
    }
}

fn run_service() -> Result<(), Box<dyn std::error::Error>> {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handler = move |control| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;

    let set_status = |state: ServiceState, accept: ServiceControlAccept| {
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })
    };

    set_status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
    )?;

    // Bring up the server on a runtime; SCM Stop drops the runtime (aborts it).
    let cfg = crate::config::Config::load(
        std::env::var(CONFIG_ENV).ok().map(PathBuf::from).as_deref(),
    )?;
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.spawn(async move {
        if let Err(e) = crate::run_server(cfg).await {
            tracing::error!(error = %e, "run_server exited");
        }
    });

    let _ = stop_rx.recv(); // block until SCM asks us to stop
    set_status(ServiceState::Stopped, ServiceControlAccept::empty())?;
    Ok(())
}

/// Register the service with the SCM (binPath = `<exe> run-service --config <abs>`).
pub fn install(exe: &Path, config_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;
    let config_abs = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.to_path_buf());
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments: vec![
            OsString::from("run-service"),
            OsString::from("--config"),
            config_abs.into_os_string(),
        ],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    manager.create_service(&info, ServiceAccess::empty())?;
    Ok(())
}

/// Start the installed service.
pub fn start() -> Result<(), Box<dyn std::error::Error>> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service = manager.open_service(SERVICE_NAME, ServiceAccess::START)?;
    service.start(&[] as &[&OsStr])?;
    Ok(())
}
