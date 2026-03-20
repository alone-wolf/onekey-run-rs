use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{
    HookName, ResolvedActionConfig, ResolvedLogConfig, ResolvedServiceConfig,
};
use crate::error::{AppError, AppResult};
use crate::file_log::{FileLogSink, SharedFileLogSink};
use crate::runtime_state::{PlatformRuntimeState, ServiceRuntimeState};

pub struct SpawnedProcess {
    pub child: Child,
    pub state: ServiceRuntimeState,
}

#[derive(Clone, Debug)]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug)]
pub struct LogEvent {
    pub service_name: String,
    pub stream: LogStream,
    pub line: String,
}

pub enum OutputMode {
    Null,
    Capture(CaptureOptions),
}

pub struct CaptureOptions {
    pub event_sender: Option<Sender<LogEvent>>,
    pub log: Option<ResolvedLogConfig>,
    pub echo_to_terminal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionRunStatus {
    Succeeded,
    Failed { status: String },
    TimedOut { timeout_secs: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopOutcome {
    Graceful,
    TimedOutForced,
    Forced,
}

pub fn run_action(
    action: &ResolvedActionConfig,
    service_name: &str,
    hook_name: HookName,
) -> AppResult<ActionRunStatus> {
    let mut command = Command::new(&action.executable);
    command
        .args(&action.args)
        .current_dir(&action.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .envs(&action.env);

    configure_command(&mut command);

    let mut child = command.spawn().map_err(|error| {
        AppError::startup_failed(format!(
            "failed to start action `{}` for service `{}` hook `{}` with executable `{}`: {error}",
            action.name,
            service_name,
            hook_name.as_str(),
            action.executable
        ))
    })?;

    let started_at = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            AppError::startup_failed(format!(
                "failed to inspect action `{}` for service `{}` hook `{}`: {error}",
                action.name,
                service_name,
                hook_name.as_str()
            ))
        })? {
            if status.success() {
                return Ok(ActionRunStatus::Succeeded);
            }

            return Ok(ActionRunStatus::Failed {
                status: status.to_string(),
            });
        }

        if let Some(timeout_secs) = action.timeout_secs
            && started_at.elapsed() >= Duration::from_secs(timeout_secs)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(ActionRunStatus::TimedOut { timeout_secs });
        }

        thread::sleep(Duration::from_millis(50));
    }
}

pub fn spawn_service(
    service: &ResolvedServiceConfig,
    output_mode: OutputMode,
) -> AppResult<SpawnedProcess> {
    let mut command = Command::new(&service.executable);
    command
        .args(&service.args)
        .current_dir(&service.cwd)
        .stdin(Stdio::null())
        .envs(&service.env);

    match &output_mode {
        OutputMode::Null => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        OutputMode::Capture(_) => {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
    }

    configure_command(&mut command);

    let mut child = command.spawn().map_err(|error| {
        AppError::startup_failed(format!(
            "failed to start service `{}` with executable `{}`: {error}",
            service.name, service.executable
        ))
    })?;

    if let Some(status) = child.try_wait().map_err(|error| {
        AppError::startup_failed(format!(
            "failed to inspect startup status for service `{}`: {error}",
            service.name
        ))
    })? {
        return Err(AppError::startup_failed(format!(
            "service `{}` exited immediately with status {status}",
            service.name
        )));
    }

    let pid = child.id();
    if let OutputMode::Capture(options) = output_mode {
        let log_sink = match options.log {
            Some(log) => Some(FileLogSink::open_shared(log)?),
            None => None,
        };
        if let Some(stdout) = child.stdout.take() {
            spawn_log_reader(
                service.name.clone(),
                LogStream::Stdout,
                stdout,
                options.event_sender.clone(),
                log_sink.clone(),
                options.echo_to_terminal,
            );
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_reader(
                service.name.clone(),
                LogStream::Stderr,
                stderr,
                options.event_sender,
                log_sink,
                options.echo_to_terminal,
            );
        }
    }

    let state = ServiceRuntimeState {
        service_name: service.name.clone(),
        pid,
        cwd: service.cwd.clone(),
        executable: service.executable.clone(),
        args: service.args.clone(),
        log_file: service.log.as_ref().map(|log| log.file.clone()),
        stop_signal: service.stop_signal.clone(),
        stop_timeout_secs: service.stop_timeout_secs,
        platform: platform_runtime_state(pid),
    };

    Ok(SpawnedProcess { child, state })
}

fn spawn_log_reader<R>(
    service_name: String,
    stream: LogStream,
    reader: R,
    sender: Option<Sender<LogEvent>>,
    log_sink: Option<SharedFileLogSink>,
    echo_to_terminal: bool,
) where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                    if let Some(sender) = &sender {
                        let _ = sender.send(LogEvent {
                            service_name: service_name.clone(),
                            stream: stream.clone(),
                            line: trimmed.clone(),
                        });
                    }
                    if let Some(log_sink) = &log_sink {
                        let prefix = match stream {
                            LogStream::Stdout => "out",
                            LogStream::Stderr => "err",
                        };
                        if let Ok(mut log_sink) = log_sink.lock() {
                            let _ = log_sink.write_line(&format!("[{prefix}] {trimmed}"));
                        }
                    }
                    if echo_to_terminal {
                        match stream {
                            LogStream::Stdout => println!("[{}] {}", service_name, trimmed),
                            LogStream::Stderr => eprintln!("[{}][err] {}", service_name, trimmed),
                        }
                    }
                }
                Err(error) => {
                    if let Some(sender) = &sender {
                        let _ = sender.send(LogEvent {
                            service_name: service_name.clone(),
                            stream: LogStream::Stderr,
                            line: format!("[onekey-run] failed to read process output: {error}"),
                        });
                    }
                    break;
                }
            }
        }
    });
}

pub fn stop_service_with_outcome(
    state: &ServiceRuntimeState,
    force: bool,
) -> AppResult<StopOutcome> {
    if !is_pid_alive(state.pid) {
        return Ok(StopOutcome::Graceful);
    }

    if force {
        force_stop_service(state)?;
        return Ok(StopOutcome::Forced);
    }

    request_stop_service(state)?;
    wait_until_stopped_outcome(state, Duration::from_secs(state.stop_timeout_secs))
}

pub fn wait_until_stopped_outcome(
    state: &ServiceRuntimeState,
    timeout: Duration,
) -> AppResult<StopOutcome> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !is_pid_alive(state.pid) {
            return Ok(StopOutcome::Graceful);
        }
        thread::sleep(Duration::from_millis(200));
    }

    force_stop_service(state)?;
    if is_pid_alive(state.pid) {
        return Err(AppError::shutdown_timed_out(format!(
            "service `{}` did not stop within {} seconds",
            state.service_name, state.stop_timeout_secs
        )));
    }

    Ok(StopOutcome::TimedOutForced)
}

pub fn is_pid_alive(pid: u32) -> bool {
    is_pid_alive_impl(pid)
}

pub fn validate_process_identity(
    project_root: &Path,
    state: &crate::runtime_state::RuntimeState,
) -> AppResult<()> {
    if state.project_root != project_root {
        return Err(AppError::runtime_failed(format!(
            "runtime state belongs to {}, not current directory {}",
            state.project_root.display(),
            project_root.display()
        )));
    }
    Ok(())
}

pub fn service_exited(child: &mut Child) -> AppResult<Option<std::process::ExitStatus>> {
    child
        .try_wait()
        .map_err(|error| AppError::runtime_failed(format!("failed to poll child process: {error}")))
}

pub fn request_stop_service(state: &ServiceRuntimeState) -> AppResult<()> {
    request_stop_impl(state)
}

pub fn force_stop_service(state: &ServiceRuntimeState) -> AppResult<()> {
    force_stop_impl(state)
}

#[cfg(unix)]
fn configure_command(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(windows)]
fn configure_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(unix)]
fn platform_runtime_state(pid: u32) -> PlatformRuntimeState {
    PlatformRuntimeState {
        process_group_id: Some(pid as i32),
    }
}

#[cfg(windows)]
fn platform_runtime_state(_pid: u32) -> PlatformRuntimeState {
    PlatformRuntimeState::default()
}

#[cfg(unix)]
fn is_pid_alive_impl(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    match kill(Pid::from_raw(pid as i32), None::<Signal>) {
        Ok(()) => true,
        Err(Errno::EPERM) => true,
        Err(Errno::ESRCH) => false,
        Err(_) => false,
    }
}

#[cfg(windows)]
fn is_pid_alive_impl(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return false;
        }

        let mut exit_code = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && exit_code == STILL_ACTIVE
    }
}

#[cfg(unix)]
fn request_stop_impl(state: &ServiceRuntimeState) -> AppResult<()> {
    use nix::sys::signal::Signal;

    let signal = match state.stop_signal.as_deref() {
        Some("sigint") | Some("SIGINT") | Some("int") | Some("INT") => Signal::SIGINT,
        Some("sigkill") | Some("SIGKILL") | Some("kill") | Some("KILL") => Signal::SIGKILL,
        _ => Signal::SIGTERM,
    };
    signal_unix_target(state, signal).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to send stop signal to service `{}` (pid {}): {error}",
            state.service_name, state.pid
        ))
    })
}

#[cfg(unix)]
fn force_stop_impl(state: &ServiceRuntimeState) -> AppResult<()> {
    use nix::sys::signal::Signal;

    signal_unix_target(state, Signal::SIGKILL).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to send SIGKILL to service `{}` (pid {}): {error}",
            state.service_name, state.pid
        ))
    })
}

#[cfg(unix)]
fn signal_unix_target(
    state: &ServiceRuntimeState,
    signal: nix::sys::signal::Signal,
) -> nix::Result<()> {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    if let Some(group) = state.platform.process_group_id {
        if kill(Pid::from_raw(-group), signal).is_ok() {
            return Ok(());
        }
    }

    kill(Pid::from_raw(state.pid as i32), signal)
}

#[cfg(windows)]
fn request_stop_impl(state: &ServiceRuntimeState) -> AppResult<()> {
    let status = Command::new("taskkill")
        .args(["/PID", &state.pid.to_string(), "/T"])
        .status()
        .map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to invoke taskkill for service `{}` (pid {}): {error}",
                state.service_name, state.pid
            ))
        })?;

    if !status.success() && is_pid_alive(state.pid) {
        return Err(AppError::runtime_failed(format!(
            "taskkill failed while stopping service `{}` (pid {})",
            state.service_name, state.pid
        )));
    }

    Ok(())
}

#[cfg(windows)]
fn force_stop_impl(state: &ServiceRuntimeState) -> AppResult<()> {
    let status = Command::new("taskkill")
        .args(["/PID", &state.pid.to_string(), "/T", "/F"])
        .status()
        .map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to force-stop service `{}` (pid {}): {error}",
                state.service_name, state.pid
            ))
        })?;

    if !status.success() && is_pid_alive(state.pid) {
        return Err(AppError::runtime_failed(format!(
            "taskkill /F failed while stopping service `{}` (pid {})",
            state.service_name, state.pid
        )));
    }

    Ok(())
}
