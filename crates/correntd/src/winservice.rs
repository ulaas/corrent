//! `correntd service install|uninstall|run`: Corrent Server as a Windows service.
//!
//! `install` registers the service (automatic start, restarted after failures) to run
//! `correntd.exe service run --data-dir <dir>`; data defaults to `%ProgramData%\Corrent Server`.
//! The service runs as LocalSystem unless `--account`/`--password` name a user (needed when
//! media folders live in a user's profile or on network shares).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceErrorControl,
    ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

pub const NAME: &str = "CorrentServer";
const DISPLAY: &str = "Corrent Server";

/// The server's main function and the way to ask it to stop (set by `main`).
pub struct Hooks {
    pub run: fn() -> anyhow::Result<()>,
    pub stop: fn(),
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

pub fn default_data_dir() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData")).join(DISPLAY)
}

pub fn install(data_dir: Option<PathBuf>, account: Option<String>, password: Option<String>) -> anyhow::Result<()> {
    let data_dir = data_dir.unwrap_or_else(default_data_dir);
    std::fs::create_dir_all(&data_dir)?;
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )?;
    let info = ServiceInfo {
        name: OsString::from(NAME),
        display_name: OsString::from(DISPLAY),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe()?,
        launch_arguments: vec![
            OsString::from("service"),
            OsString::from("run"),
            OsString::from("--data-dir"),
            data_dir.clone().into_os_string(),
        ],
        dependencies: vec![],
        account_name: account.map(OsString::from),
        account_password: password.map(OsString::from),
    };
    let service = manager.create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)?;
    service.set_description("Corrent's media and torrent server (web interface, API, streaming).")?;
    // Restart after a crash: 5 s, 30 s, then every minute; the count resets after a day.
    let restart = |secs| ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(secs) };
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart(5), restart(30), restart(60)]),
    })?;
    service.start::<&str>(&[])?;
    println!("installed and started the {DISPLAY} service (data in {})", data_dir.display());
    Ok(())
}

pub fn uninstall() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let service =
        manager.open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        let _ = service.stop();
        for _ in 0..60 {
            if service.query_status()?.current_state == ServiceState::Stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    service.delete()?;
    println!("removed the {DISPLAY} service (its data folder is kept)");
    Ok(())
}

/// Hands the process to the service control manager (blocks until the service stops).
pub fn run(hooks: Hooks) -> anyhow::Result<()> {
    let _ = HOOKS.set(hooks);
    service_dispatcher::start(NAME, ffi_service_main)?;
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    let Some(hooks) = HOOKS.get() else { return };
    let stop = hooks.stop;
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
            stop();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(status) = service_control_handler::register(NAME, handler) else { return };
    let set = |state, accept, code: u32, wait| {
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(code),
            checkpoint: 0,
            wait_hint: wait,
            process_id: None,
        });
    };
    set(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN, 0, Duration::ZERO);
    let result = (hooks.run)();
    if let Err(e) = &result {
        tracing::error!("service: {e:#}");
    }
    // ERROR_SERVICE_SPECIFIC_ERROR (1066) tells the manager to apply the failure actions.
    set(ServiceState::Stopped, ServiceControlAccept::empty(), if result.is_ok() { 0 } else { 1066 }, Duration::ZERO);
}
