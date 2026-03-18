use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{LogOverflowStrategy, ResolvedLogConfig, ResolvedServiceConfig};
use crate::error::{AppError, AppResult};
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
            Some(log) => Some(LogSink::open(log)?),
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

type SharedLogSink = Arc<Mutex<LogSink>>;

fn spawn_log_reader<R>(
    service_name: String,
    stream: LogStream,
    reader: R,
    sender: Option<Sender<LogEvent>>,
    log_sink: Option<SharedLogSink>,
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
                            let _ = log_sink.write_line(prefix, &trimmed);
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

struct LogSink {
    config: ResolvedLogConfig,
    writer: Option<std::io::BufWriter<std::fs::File>>,
    current_size: u64,
    archive_sequence: u64,
}

impl LogSink {
    fn open(config: ResolvedLogConfig) -> AppResult<SharedLogSink> {
        let (writer, current_size) = open_log_writer(&config.file, config.append)?;
        Ok(Arc::new(Mutex::new(Self {
            config,
            writer: Some(writer),
            current_size,
            archive_sequence: 0,
        })))
    }

    fn write_line(&mut self, prefix: &str, line: &str) -> AppResult<()> {
        let payload = format!("[{prefix}] {line}\n");
        let payload_size = payload.len() as u64;

        if let Some(max_file_bytes) = self.config.max_file_bytes
            && self.current_size > 0
            && self.current_size.saturating_add(payload_size) > max_file_bytes
        {
            self.overflow()?;
        }

        let writer = self
            .writer
            .as_mut()
            .expect("log sink writer must be available while writing");
        std::io::Write::write_all(writer, payload.as_bytes()).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to write log file {}: {error}",
                self.config.file.display()
            ))
        })?;
        std::io::Write::flush(writer).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to flush log file {}: {error}",
                self.config.file.display()
            ))
        })?;
        self.current_size = self.current_size.saturating_add(payload_size);
        Ok(())
    }

    fn overflow(&mut self) -> AppResult<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.flush().map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to flush log file {} before overflow handling: {error}",
                    self.config.file.display()
                ))
            })?;
        }
        let writer = self.writer.take();
        drop(writer);

        match self.config.overflow_strategy {
            Some(LogOverflowStrategy::Rotate) => self.rotate_files()?,
            Some(LogOverflowStrategy::Archive) => self.archive_file()?,
            None => unreachable!("validated log configuration must set overflow_strategy"),
        }

        let (writer, size) = open_log_writer(&self.config.file, false)?;
        self.writer = Some(writer);
        self.current_size = size;
        Ok(())
    }

    fn rotate_files(&mut self) -> AppResult<()> {
        let rotate_count = self
            .config
            .rotate_file_count
            .expect("validated rotate configuration must include rotate_file_count");

        let oldest = rotate_path(&self.config.file, rotate_count);
        if oldest.exists() {
            fs::remove_file(&oldest).map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to remove rotated log file {}: {error}",
                    oldest.display()
                ))
            })?;
        }

        for index in (1..rotate_count).rev() {
            let source = rotate_path(&self.config.file, index);
            let target = rotate_path(&self.config.file, index + 1);
            if source.exists() {
                fs::rename(&source, &target).map_err(|error| {
                    AppError::runtime_failed(format!(
                        "failed to rename rotated log file {} to {}: {error}",
                        source.display(),
                        target.display()
                    ))
                })?;
            }
        }

        if self.config.file.exists() {
            let first = rotate_path(&self.config.file, 1);
            fs::rename(&self.config.file, &first).map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to rotate active log file {} to {}: {error}",
                    self.config.file.display(),
                    first.display()
                ))
            })?;
        }

        Ok(())
    }

    fn archive_file(&mut self) -> AppResult<()> {
        if !self.config.file.exists() {
            return Ok(());
        }

        self.archive_sequence = self.archive_sequence.saturating_add(1);
        let archive_path = archive_path(&self.config.file, self.archive_sequence);
        fs::rename(&self.config.file, &archive_path).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to archive log file {} to {}: {error}",
                self.config.file.display(),
                archive_path.display()
            ))
        })?;
        Ok(())
    }
}

fn open_log_writer(
    path: &Path,
    append: bool,
) -> AppResult<(std::io::BufWriter<std::fs::File>, u64)> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::startup_failed(format!(
                "failed to create log directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let existing_size = if append && path.exists() {
        path.metadata().map(|metadata| metadata.len()).unwrap_or(0)
    } else {
        0
    };

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)
        .map_err(|error| {
            AppError::startup_failed(format!(
                "failed to open log file {}: {error}",
                path.display()
            ))
        })?;

    Ok((std::io::BufWriter::new(file), existing_size))
}

fn rotate_path(path: &Path, index: usize) -> std::path::PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(format!(".{index}"));
    std::path::PathBuf::from(os)
}

fn archive_path(path: &Path, sequence: u64) -> std::path::PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time must be after unix epoch")
        .as_millis();
    let mut os = path.as_os_str().to_os_string();
    os.push(format!(".{millis}.{sequence:03}"));
    std::path::PathBuf::from(os)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config::{LogOverflowStrategy, ResolvedLogConfig};

    use super::LogSink;

    #[test]
    fn rotate_creates_numbered_history_files() {
        let dir = temp_dir("rotate");
        let log_path = dir.join("app.log");
        let sink = LogSink::open(ResolvedLogConfig {
            file: log_path.clone(),
            append: true,
            max_file_bytes: Some(25),
            overflow_strategy: Some(LogOverflowStrategy::Rotate),
            rotate_file_count: Some(2),
        })
        .unwrap();

        {
            let mut sink = sink.lock().unwrap();
            sink.write_line("out", "1234567890").unwrap();
            sink.write_line("out", "abcdefghij").unwrap();
            sink.write_line("out", "klmnopqrst").unwrap();
        }

        assert!(log_path.exists());
        assert!(PathBuf::from(format!("{}.1", log_path.display())).exists());
        assert!(!fs::read_to_string(&log_path).unwrap().is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn archive_creates_additional_files() {
        let dir = temp_dir("archive");
        let log_path = dir.join("worker.log");
        let sink = LogSink::open(ResolvedLogConfig {
            file: log_path.clone(),
            append: true,
            max_file_bytes: Some(25),
            overflow_strategy: Some(LogOverflowStrategy::Archive),
            rotate_file_count: None,
        })
        .unwrap();

        {
            let mut sink = sink.lock().unwrap();
            sink.write_line("out", "1234567890").unwrap();
            sink.write_line("out", "abcdefghij").unwrap();
            sink.write_line("out", "klmnopqrst").unwrap();
        }

        let archived = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("worker.log.")
            })
            .count();
        assert!(archived >= 1);

        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onekey-run-rs-process-{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

pub fn stop_service(state: &ServiceRuntimeState, force: bool) -> AppResult<()> {
    if !is_pid_alive(state.pid) {
        return Ok(());
    }

    if force {
        force_stop_service(state)?;
        return Ok(());
    }

    request_stop_service(state)?;
    wait_until_stopped(state, Duration::from_secs(state.stop_timeout_secs))?;
    Ok(())
}

pub fn wait_until_stopped(state: &ServiceRuntimeState, timeout: Duration) -> AppResult<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !is_pid_alive(state.pid) {
            return Ok(());
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

    Ok(())
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
