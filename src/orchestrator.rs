use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::{ProjectConfig, ResolvedServiceConfig};
use crate::error::{AppError, AppResult};
use crate::process::{self, CaptureOptions, OutputMode, SpawnedProcess};
use crate::runtime_state::{self, RegistryEntry, RuntimeLock, RuntimeState};

pub struct RunPlan {
    pub project_root: PathBuf,
    pub config_path: PathBuf,
    pub services: Vec<ResolvedServiceConfig>,
}

pub struct RunOptions {
    pub tui: bool,
    pub daemonized: bool,
}

const INIT_TEMPLATE: &str = r#"defaults:
  stop_timeout_secs: 10

services:
  app:
    executable: "sleep"
    args: ["30"]
    cwd: "."
    log:
      file: "./logs/app.log"
      append: true
      max_file_bytes: 10485760
      overflow_strategy: "rotate"
      rotate_file_count: 5

  worker:
    executable: "sleep"
    args: ["30"]
    cwd: "."
    depends_on: ["app"]
    log:
      file: "./logs/worker.log"
      append: true
      max_file_bytes: 10485760
      overflow_strategy: "rotate"
      rotate_file_count: 5
"#;

const INIT_TEMPLATE_FULL: &str = r#"defaults:
  stop_timeout_secs: 10
  restart: "no"

services:
  app:
    executable: "sleep"
    args: ["30"]
    cwd: "."
    env:
      RUST_LOG: "info"
    log:
      file: "./logs/app.log"
      append: true
      max_file_bytes: 10485760
      overflow_strategy: "rotate"
      rotate_file_count: 5
    depends_on: []
    restart: "no"
    stop_signal: "term"
    stop_timeout_secs: 10
    autostart: true
    disabled: false

  worker:
    executable: "sleep"
    args: ["30"]
    cwd: "."
    env: {}
    log:
      file: "./logs/worker.log"
      append: true
      max_file_bytes: 10485760
      overflow_strategy: "rotate"
      rotate_file_count: 5
    depends_on: ["app"]
    restart: "on-failure"
    stop_signal: "term"
    stop_timeout_secs: 10
    autostart: true
    disabled: false
"#;

pub fn run_init(config_path: &Path, full: bool) -> AppResult<()> {
    if config_path.exists() {
        return Err(AppError::startup_failed(format!(
            "refusing to overwrite existing configuration at {}",
            config_path.display()
        )));
    }

    if let Some(parent) = config_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::startup_failed(format!(
                "failed to create configuration directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let template = if full {
        INIT_TEMPLATE_FULL
    } else {
        INIT_TEMPLATE
    };

    fs::write(config_path, template).map_err(|error| {
        AppError::startup_failed(format!(
            "failed to write configuration template to {}: {error}",
            config_path.display()
        ))
    })?;

    println!(
        "created configuration template at {}",
        config_path.display()
    );
    Ok(())
}

pub fn build_run_plan(
    config: &ProjectConfig,
    config_path: &Path,
    targets: &[String],
) -> AppResult<RunPlan> {
    let config_path = canonical_or_owned(config_path)?;
    let project_root = config_path
        .parent()
        .ok_or_else(|| AppError::config_invalid("configuration file must have a parent directory"))?
        .to_path_buf();

    let selected = select_services(config, targets)?;
    let ordered_names = topo_sort(config, &selected)?;
    let mut services = Vec::with_capacity(ordered_names.len());
    for name in ordered_names {
        services.push(config.resolve_service(&name, &project_root)?);
    }

    Ok(RunPlan {
        project_root,
        config_path,
        services,
    })
}

pub fn run_check(plan: &RunPlan, config: &ProjectConfig) -> AppResult<()> {
    for service in &plan.services {
        config.executable_exists(&service.name, &plan.project_root)?;
    }

    println!(
        "configuration ok: {} service(s) validated from {}",
        plan.services.len(),
        plan.config_path.display()
    );
    Ok(())
}

pub fn run_up(plan: RunPlan, options: RunOptions) -> AppResult<()> {
    if options.tui {
        return run_up_tui(plan);
    }
    if options.daemonized {
        return run_up_daemonized(plan);
    }
    run_up_plain(plan)
}

fn run_up_plain(plan: RunPlan) -> AppResult<()> {
    let lock = RuntimeLock::acquire(&plan.project_root)?;
    let mut runtime_state = RuntimeState::new(plan.project_root.clone(), plan.config_path.clone());
    runtime_state::write_state(&plan.project_root, &runtime_state)?;

    let shutdown = install_shutdown_controller()?;

    let mut running = Vec::new();
    for service in &plan.services {
        let output_mode = if service.log.is_some() {
            OutputMode::Capture(CaptureOptions {
                event_sender: None,
                log: service.log.clone(),
                echo_to_terminal: false,
            })
        } else {
            OutputMode::Null
        };

        let spawned = match process::spawn_service(service, output_mode) {
            Ok(spawned) => spawned,
            Err(error) => {
                let shutdown_result = shutdown_running_services(
                    &mut running,
                    &ShutdownController::force_requested_now(),
                );
                let cleanup_result = runtime_state::cleanup_runtime_files(&plan.project_root)
                    .and_then(|_| lock.release());
                shutdown_result?;
                cleanup_result?;
                return Err(error);
            }
        };
        runtime_state.services.push(spawned.state.clone());
        runtime_state::write_state(&plan.project_root, &runtime_state)?;
        running.push(spawned);
    }
    runtime_state::register_instance(&runtime_state)?;

    render_plain_status(&running, Duration::from_secs(0))?;

    let runtime_result = monitor_plain_processes(&plan.project_root, &mut running, &shutdown);
    let shutdown_result = shutdown_running_services(&mut running, &shutdown);

    let cleanup_result =
        runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());

    println!();
    runtime_result?;
    shutdown_result?;
    cleanup_result?;

    Ok(())
}

fn run_up_tui(plan: RunPlan) -> AppResult<()> {
    let lock = RuntimeLock::acquire(&plan.project_root)?;
    let mut runtime_state = RuntimeState::new(plan.project_root.clone(), plan.config_path.clone());
    runtime_state::write_state(&plan.project_root, &runtime_state)?;

    let shutdown = install_shutdown_controller()?;

    let (log_tx, log_rx) = mpsc::channel();
    let mut running = Vec::new();
    for service in &plan.services {
        let spawned = match process::spawn_service(
            service,
            OutputMode::Capture(CaptureOptions {
                event_sender: Some(log_tx.clone()),
                log: service.log.clone(),
                echo_to_terminal: false,
            }),
        ) {
            Ok(spawned) => spawned,
            Err(error) => {
                let shutdown_result = shutdown_running_services(
                    &mut running,
                    &ShutdownController::force_requested_now(),
                );
                let cleanup_result = runtime_state::cleanup_runtime_files(&plan.project_root)
                    .and_then(|_| lock.release());
                shutdown_result?;
                cleanup_result?;
                return Err(error);
            }
        };
        runtime_state.services.push(spawned.state.clone());
        runtime_state::write_state(&plan.project_root, &runtime_state)?;
        running.push(spawned);
    }
    runtime_state::register_instance(&runtime_state)?;
    drop(log_tx);

    let runtime_result =
        crate::tui::run_dashboard(&plan.project_root, &mut running, log_rx, shutdown.clone());
    let shutdown_result = shutdown_running_services(&mut running, &shutdown);
    let cleanup_result =
        runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());

    runtime_result?;
    shutdown_result?;
    cleanup_result?;

    Ok(())
}

fn run_up_daemonized(plan: RunPlan) -> AppResult<()> {
    let lock = RuntimeLock::acquire(&plan.project_root)?;
    let mut runtime_state = RuntimeState::new(plan.project_root.clone(), plan.config_path.clone());
    runtime_state::write_state(&plan.project_root, &runtime_state)?;

    let shutdown = install_shutdown_controller()?;

    let mut running = Vec::new();
    for service in &plan.services {
        let output_mode = if service.log.is_some() {
            OutputMode::Capture(CaptureOptions {
                event_sender: None,
                log: service.log.clone(),
                echo_to_terminal: false,
            })
        } else {
            OutputMode::Null
        };

        let spawned = match process::spawn_service(service, output_mode) {
            Ok(spawned) => spawned,
            Err(error) => {
                let shutdown_result = shutdown_running_services(
                    &mut running,
                    &ShutdownController::force_requested_now(),
                );
                let cleanup_result = runtime_state::cleanup_runtime_files(&plan.project_root)
                    .and_then(|_| lock.release());
                shutdown_result?;
                cleanup_result?;
                return Err(error);
            }
        };
        runtime_state.services.push(spawned.state.clone());
        runtime_state::write_state(&plan.project_root, &runtime_state)?;
        running.push(spawned);
    }
    runtime_state::register_instance(&runtime_state)?;

    let runtime_result = monitor_daemon_processes(&plan.project_root, &mut running, &shutdown);
    let shutdown_result = shutdown_running_services(&mut running, &shutdown);
    let cleanup_result =
        runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());

    runtime_result?;
    shutdown_result?;
    cleanup_result?;

    Ok(())
}

pub fn run_down(project_root: &Path, force: bool) -> AppResult<()> {
    let state = runtime_state::load_state(project_root)?;
    process::validate_process_identity(project_root, &state)?;

    if state.services.is_empty() {
        runtime_state::cleanup_runtime_files(project_root)?;
        println!("no running services recorded");
        return Ok(());
    }

    let mut first_error = None;
    for service in state.services.iter().rev() {
        println!("stopping {}", service.service_name);
        if let Err(error) = process::stop_service(service, force) {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }

    if process::is_pid_alive(state.tool_pid) && state.tool_pid != std::process::id() {
        #[cfg(unix)]
        {
            use nix::sys::signal::{Signal, kill};
            use nix::unistd::Pid;
            let _ = kill(Pid::from_raw(state.tool_pid as i32), Signal::SIGTERM);
        }

        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &state.tool_pid.to_string(), "/F"])
                .status();
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }

    runtime_state::cleanup_runtime_files(project_root)?;

    println!("services stopped");
    Ok(())
}

pub fn run_management(watch: bool, json: bool) -> AppResult<()> {
    if watch {
        return run_management_watch();
    }

    let active = collect_management_entries()?;
    if json {
        print_management_json(&active)?;
    } else {
        print_management_entries(&active)?;
    }
    Ok(())
}

fn run_management_watch() -> AppResult<()> {
    let shutdown = install_shutdown_controller()?;

    loop {
        let active = collect_management_entries()?;
        print!("\x1b[2J\x1b[H");
        io::stdout().flush().map_err(|error| {
            AppError::runtime_failed(format!("failed to flush management watch output: {error}"))
        })?;
        print_management_entries(&active)?;
        println!();
        println!("watching onekey instances... press Ctrl-C to exit");

        if shutdown.shutdown_requested() {
            println!();
            println!("management watch stopped");
            return Ok(());
        }

        thread::sleep(Duration::from_secs(1));
    }
}

fn collect_management_entries() -> AppResult<Vec<ManagementEntry>> {
    let mut entries = runtime_state::list_registry_entries()?;
    entries.sort_by(|left, right| left.project_root.cmp(&right.project_root));

    let mut active = Vec::new();
    for entry in entries {
        let state_exists = runtime_state::state_path(&entry.project_root).exists();
        let lock_exists = entry
            .project_root
            .join(runtime_state::RUNTIME_DIR)
            .join(runtime_state::LOCK_FILE)
            .exists();

        let tool_alive = process::is_pid_alive(entry.tool_pid);
        let runtime_state = if state_exists {
            runtime_state::load_state(&entry.project_root).ok()
        } else {
            None
        };
        let service_names = runtime_state
            .as_ref()
            .map(|state| {
                state
                    .services
                    .iter()
                    .map(|service| service.service_name.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| entry.service_names.clone());
        let service_count = service_names.len();
        let alive_services = runtime_state
            .as_ref()
            .map(|state| {
                state
                    .services
                    .iter()
                    .filter(|service| process::is_pid_alive(service.pid))
                    .count()
            })
            .unwrap_or(0);

        if tool_alive || state_exists || lock_exists {
            active.push(ManagementEntry::from_registry_entry(
                entry,
                tool_alive,
                alive_services,
                service_count,
                service_names,
                state_exists || lock_exists,
            ));
        } else {
            let _ = runtime_state::cleanup_runtime_files(&entry.project_root);
        }
    }

    Ok(active)
}

fn print_management_entries(active: &[ManagementEntry]) -> AppResult<()> {
    if active.is_empty() {
        println!("no running onekey instances");
        return Ok(());
    }

    println!("onekey instances: {}", active.len());
    for entry in active {
        let services = if entry.service_names.is_empty() {
            "-".to_owned()
        } else {
            entry.service_names.join(", ")
        };
        println!(
            "- pid {} | status {} | uptime {} | root {} | config {} | services: {}",
            entry.tool_pid,
            entry.status_summary,
            entry.uptime,
            entry.project_root.display(),
            entry.config_path.display(),
            services
        );
    }

    Ok(())
}

fn print_management_json(active: &[ManagementEntry]) -> AppResult<()> {
    let snapshot = ManagementSnapshot {
        generated_at_unix_secs: current_unix_secs(),
        instance_count: active.len(),
        instances: active.to_vec(),
    };
    let raw = serde_json::to_string_pretty(&snapshot).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to serialize management json output: {error}"
        ))
    })?;
    println!("{raw}");
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct ManagementSnapshot {
    generated_at_unix_secs: u64,
    instance_count: usize,
    instances: Vec<ManagementEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct ManagementEntry {
    tool_pid: u32,
    project_root: PathBuf,
    config_path: PathBuf,
    service_names: Vec<String>,
    service_count: usize,
    alive_services: usize,
    started_at_unix_secs: u64,
    uptime_secs: u64,
    uptime: String,
    status_summary: String,
}

impl ManagementEntry {
    fn from_registry_entry(
        entry: RegistryEntry,
        tool_alive: bool,
        alive_services: usize,
        service_count: usize,
        service_names: Vec<String>,
        has_runtime_artifacts: bool,
    ) -> Self {
        let uptime_secs = current_unix_secs().saturating_sub(entry.started_at_unix_secs);
        let status_summary = summarize_instance_status(
            tool_alive,
            alive_services,
            service_count,
            has_runtime_artifacts,
        );

        Self {
            tool_pid: entry.tool_pid,
            project_root: entry.project_root,
            config_path: entry.config_path,
            service_names,
            service_count,
            alive_services,
            started_at_unix_secs: entry.started_at_unix_secs,
            uptime_secs,
            uptime: format_elapsed(Duration::from_secs(uptime_secs)),
            status_summary,
        }
    }
}

fn summarize_instance_status(
    tool_alive: bool,
    alive_services: usize,
    service_count: usize,
    has_runtime_artifacts: bool,
) -> String {
    if service_count > 0 && alive_services == service_count && tool_alive {
        return "running".to_owned();
    }

    if tool_alive || alive_services > 0 {
        return format!("partial ({alive_services}/{service_count} alive)");
    }

    if has_runtime_artifacts {
        return "stale".to_owned();
    }

    "unknown".to_owned()
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time must be after unix epoch")
        .as_secs()
}

fn monitor_plain_processes(
    project_root: &Path,
    running: &mut [SpawnedProcess],
    shutdown: &ShutdownController,
) -> AppResult<()> {
    let started_at = std::time::Instant::now();
    let mut last_rendered_secs = 0;

    loop {
        if shutdown.shutdown_requested() {
            println!();
            eprintln!("interrupt received, stopping services. press Ctrl-C again to force.");
            return Ok(());
        }

        for process in running.iter_mut() {
            if let Some(status) = process::service_exited(&mut process.child)? {
                if !runtime_state::state_path(project_root).exists() {
                    return Ok(());
                }
                return Err(AppError::runtime_failed(format!(
                    "service `{}` exited with status {status}",
                    process.state.service_name
                )));
            }
        }

        let elapsed = started_at.elapsed();
        if elapsed.as_secs() != last_rendered_secs {
            render_plain_status(running, elapsed)?;
            last_rendered_secs = elapsed.as_secs();
        }

        thread::sleep(Duration::from_millis(250));
    }
}

fn monitor_daemon_processes(
    project_root: &Path,
    running: &mut [SpawnedProcess],
    shutdown: &ShutdownController,
) -> AppResult<()> {
    loop {
        if shutdown.shutdown_requested() {
            return Ok(());
        }

        for process in running.iter_mut() {
            if let Some(status) = process::service_exited(&mut process.child)? {
                if !runtime_state::state_path(project_root).exists() {
                    return Ok(());
                }
                return Err(AppError::runtime_failed(format!(
                    "service `{}` exited with status {status}",
                    process.state.service_name
                )));
            }
        }

        thread::sleep(Duration::from_millis(250));
    }
}

fn render_plain_status(running: &[SpawnedProcess], elapsed: Duration) -> AppResult<()> {
    let services = running
        .iter()
        .map(|process| process.state.service_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let message = format!(
        "\r\x1b[2Krunning services: {} | elapsed: {}",
        services,
        format_elapsed(elapsed)
    );
    print!("{message}");
    io::stdout().flush().map_err(|error| {
        AppError::runtime_failed(format!("failed to flush status output: {error}"))
    })
}

fn format_elapsed(elapsed: Duration) -> String {
    let total_secs = elapsed.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn shutdown_running_services(
    running: &mut Vec<SpawnedProcess>,
    shutdown: &ShutdownController,
) -> AppResult<()> {
    let mut first_error = None;
    let mut force_notice_shown = false;
    for process in running.iter_mut().rev() {
        if let Err(error) = stop_spawned_process(process, shutdown, &mut force_notice_shown) {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(())
}

fn stop_spawned_process(
    process: &mut SpawnedProcess,
    shutdown: &ShutdownController,
    force_notice_shown: &mut bool,
) -> AppResult<()> {
    if process::service_exited(&mut process.child)?.is_some() {
        return Ok(());
    }

    if shutdown.force_requested() {
        if !*force_notice_shown {
            eprintln!("second interrupt received, force-stopping services.");
            *force_notice_shown = true;
        }
        force_kill_process(process)?;
        let _ = process.child.wait();
        return Ok(());
    }

    if let Err(error) = process::request_stop_service(&process.state) {
        return fallback_kill_process(process, Some(error));
    }

    let deadline = Instant::now() + Duration::from_secs(process.state.stop_timeout_secs);
    loop {
        if process::service_exited(&mut process.child)?.is_some() {
            return Ok(());
        }

        if shutdown.force_requested() {
            if !*force_notice_shown {
                eprintln!("second interrupt received, force-stopping services.");
                *force_notice_shown = true;
            }
            force_kill_process(process)?;
            let _ = process.child.wait();
            return Ok(());
        }

        if Instant::now() >= deadline {
            force_kill_process(process)?;
            let _ = process.child.wait();
            return Ok(());
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn force_kill_process(process: &mut SpawnedProcess) -> AppResult<()> {
    if let Err(error) = process::force_stop_service(&process.state) {
        return fallback_kill_process(process, Some(error));
    }

    Ok(())
}

fn fallback_kill_process(
    process: &mut SpawnedProcess,
    original_error: Option<AppError>,
) -> AppResult<()> {
    process
        .child
        .kill()
        .map_err(|kill_error| match original_error {
            Some(error) => AppError::runtime_failed(format!(
                "failed to stop service `{}` via signal ({error}) and child handle ({kill_error})",
                process.state.service_name
            )),
            None => AppError::runtime_failed(format!(
                "failed to force-stop service `{}` via child handle ({kill_error})",
                process.state.service_name
            )),
        })
}

fn install_shutdown_controller() -> AppResult<ShutdownController> {
    let signal_count = Arc::new(AtomicU8::new(0));
    {
        let signal_count = signal_count.clone();
        ctrlc::set_handler(move || {
            let current = signal_count.load(Ordering::SeqCst);
            if current < 2 {
                signal_count.fetch_add(1, Ordering::SeqCst);
            }
        })
        .map_err(|error| {
            AppError::startup_failed(format!("failed to install signal handler: {error}"))
        })?;
    }

    Ok(ShutdownController { signal_count })
}

#[derive(Clone)]
pub struct ShutdownController {
    signal_count: Arc<AtomicU8>,
}

impl ShutdownController {
    pub fn shutdown_requested(&self) -> bool {
        self.signal_count.load(Ordering::SeqCst) >= 1
    }

    pub fn force_requested(&self) -> bool {
        self.signal_count.load(Ordering::SeqCst) >= 2
    }

    fn force_requested_now() -> Self {
        Self {
            signal_count: Arc::new(AtomicU8::new(2)),
        }
    }
}

fn select_services(config: &ProjectConfig, targets: &[String]) -> AppResult<BTreeSet<String>> {
    let mut selected = BTreeSet::new();

    if targets.is_empty() {
        let autostart: Vec<String> = config
            .services
            .keys()
            .filter(|name| config.should_autostart(name))
            .cloned()
            .collect();

        if autostart.is_empty() {
            return Err(AppError::config_invalid(
                "configuration does not contain any autostart-enabled services",
            ));
        }

        for name in autostart {
            collect_with_dependencies(config, &name, &mut selected)?;
        }
        return Ok(selected);
    }

    for target in targets {
        collect_with_dependencies(config, target, &mut selected)?;
    }

    Ok(selected)
}

fn collect_with_dependencies(
    config: &ProjectConfig,
    name: &str,
    selected: &mut BTreeSet<String>,
) -> AppResult<()> {
    if selected.contains(name) {
        return Ok(());
    }

    let service = config.services.get(name).ok_or_else(|| {
        AppError::config_invalid(format!("service `{name}` is not defined in configuration"))
    })?;
    if service.disabled.unwrap_or(false) {
        return Err(AppError::config_invalid(format!(
            "service `{name}` is disabled and cannot be selected"
        )));
    }

    for dependency in &service.depends_on {
        collect_with_dependencies(config, dependency, selected)?;
    }

    selected.insert(name.to_owned());
    Ok(())
}

fn topo_sort(config: &ProjectConfig, selected: &BTreeSet<String>) -> AppResult<Vec<String>> {
    let mut ordered = Vec::new();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();

    for name in selected {
        topo_visit(
            config,
            name,
            selected,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    }

    Ok(ordered)
}

fn topo_visit(
    config: &ProjectConfig,
    name: &str,
    selected: &BTreeSet<String>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    ordered: &mut Vec<String>,
) -> AppResult<()> {
    if visited.contains(name) {
        return Ok(());
    }
    if !visiting.insert(name.to_owned()) {
        return Err(AppError::config_invalid(format!(
            "dependency cycle detected while planning around `{name}`"
        )));
    }

    let service = config.services.get(name).ok_or_else(|| {
        AppError::config_invalid(format!("service `{name}` is not defined in configuration"))
    })?;

    for dependency in &service.depends_on {
        if selected.contains(dependency) {
            topo_visit(config, dependency, selected, visiting, visited, ordered)?;
        }
    }

    visiting.remove(name);
    visited.insert(name.to_owned());
    ordered.push(name.to_owned());
    Ok(())
}

fn canonical_or_owned(path: &Path) -> AppResult<PathBuf> {
    if path.exists() {
        path.canonicalize()
            .map_err(|error| AppError::config_io(path, error))
    } else {
        Err(AppError::config_io(path, "file does not exist"))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{run_init, summarize_instance_status};

    #[test]
    fn init_writes_template_and_refuses_overwrite() {
        let dir = temp_dir("init-template");
        let path = dir.join("onekey-tasks.yaml");

        run_init(&path, false).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("services:"));
        assert!(raw.contains("depends_on"));
        assert!(raw.contains("log:"));
        assert!(!raw.contains("autostart: true"));

        let error = run_init(&path, false).unwrap_err();
        assert!(error.to_string().contains("refusing to overwrite"));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn init_full_writes_extended_template() {
        let dir = temp_dir("init-full-template");
        let path = dir.join("onekey-tasks.yaml");

        run_init(&path, true).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("restart: \"no\""));
        assert!(raw.contains("env:"));
        assert!(raw.contains("file: \"./logs/app.log\""));
        assert!(raw.contains("stop_signal: \"term\""));
        assert!(raw.contains("autostart: true"));
        assert!(raw.contains("disabled: false"));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn summarizes_running_instance() {
        assert_eq!(summarize_instance_status(true, 2, 2, true), "running");
    }

    #[test]
    fn summarizes_partial_instance() {
        assert_eq!(
            summarize_instance_status(true, 1, 2, true),
            "partial (1/2 alive)"
        );
    }

    #[test]
    fn summarizes_stale_instance() {
        assert_eq!(summarize_instance_status(false, 0, 2, true), "stale");
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onekey-run-rs-{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
