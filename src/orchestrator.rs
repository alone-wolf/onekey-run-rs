use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
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

use crate::config::{
    ActionRenderContext, HookName, ProjectConfig, ResolvedActionConfig, ResolvedLogConfig,
    ResolvedServiceConfig,
};
use crate::error::{AppError, AppResult};
use crate::file_log::{FileLogSink, SharedFileLogSink};
use crate::process::{
    self, ActionRunStatus, CaptureOptions, OutputMode, SpawnedProcess, StopOutcome,
};
use crate::runtime_state::{self, RegistryEntry, RuntimeEvent, RuntimeLock, RuntimeState};

pub struct RunPlan {
    pub project_root: PathBuf,
    pub config_path: PathBuf,
    pub actions: BTreeMap<String, ResolvedActionConfig>,
    pub instance_log: Option<ResolvedLogConfig>,
    pub services: Vec<ResolvedServiceConfig>,
}

pub struct RunOptions {
    pub tui: bool,
    pub daemonized: bool,
}

thread_local! {
    static INSTANCE_LOGGER: RefCell<Option<SharedFileLogSink>> = const { RefCell::new(None) };
}

struct InstanceLogGuard;

impl Drop for InstanceLogGuard {
    fn drop(&mut self) {
        INSTANCE_LOGGER.with(|logger| {
            logger.borrow_mut().take();
        });
    }
}

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
        ProjectConfig::preset_full()
    } else {
        ProjectConfig::preset_minimal()
    };

    template.validate(config_path).map_err(|error| {
        AppError::startup_failed(format!("failed to build configuration template: {error}"))
    })?;
    let raw = template.to_yaml_string().map_err(|error| {
        AppError::startup_failed(format!("failed to render configuration template: {error}"))
    })?;

    fs::write(config_path, raw).map_err(|error| {
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
    let actions = config.resolve_actions(&project_root)?;
    let instance_log = config.resolve_project_log(&project_root);
    let mut services = Vec::with_capacity(ordered_names.len());
    for name in ordered_names {
        services.push(config.resolve_service(&name, &project_root)?);
    }

    Ok(RunPlan {
        project_root,
        config_path,
        actions,
        instance_log,
        services,
    })
}

pub fn run_check(plan: &RunPlan, config: &ProjectConfig) -> AppResult<()> {
    for service in &plan.services {
        config.executable_exists(&service.name, &plan.project_root)?;
    }
    for action in plan.actions.values() {
        config.action_executable_exists(&action.name, &plan.project_root)?;
    }

    println!(
        "configuration ok: {} service(s), {} action(s) validated from {}",
        plan.services.len(),
        plan.actions.len(),
        plan.config_path.display()
    );
    Ok(())
}

fn install_instance_logger(config: Option<ResolvedLogConfig>) -> AppResult<InstanceLogGuard> {
    let sink = match config {
        Some(config) => Some(FileLogSink::open_shared(config)?),
        None => None,
    };
    INSTANCE_LOGGER.with(|logger| {
        *logger.borrow_mut() = sink;
    });
    Ok(InstanceLogGuard)
}

fn install_plan_instance_logger(
    plan: &RunPlan,
    lock: RuntimeLock,
) -> AppResult<(RuntimeLock, InstanceLogGuard)> {
    match install_instance_logger(plan.instance_log.clone()) {
        Ok(guard) => Ok((lock, guard)),
        Err(error) => {
            lock.release()?;
            Err(error)
        }
    }
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
    let (lock, _instance_log_guard) = install_plan_instance_logger(&plan, lock)?;
    let mut runtime_state = RuntimeState::new(
        plan.project_root.clone(),
        plan.config_path.clone(),
        plan.instance_log.as_ref().map(|log| log.file.clone()),
    );
    runtime_state::write_state(&plan.project_root, &runtime_state)?;
    emit_runtime_event(
        &plan.project_root,
        "instance_started",
        None,
        None,
        None,
        format_detail_fields(&[
            ("mode", "plain".to_owned()),
            ("config", plan.config_path.display().to_string()),
            ("service_count", plan.services.len().to_string()),
        ]),
    );

    let shutdown = install_shutdown_controller()?;
    let mut running = match start_services(&plan, &mut runtime_state, |service| {
        if service.log.is_some() {
            OutputMode::Capture(CaptureOptions {
                event_sender: None,
                log: service.log.clone(),
                echo_to_terminal: false,
            })
        } else {
            OutputMode::Null
        }
    }) {
        Ok(running) => running,
        Err(error) => {
            let cleanup_result =
                runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());
            cleanup_result?;
            return Err(error);
        }
    };
    runtime_state::register_instance(&runtime_state)?;

    render_plain_status(&running, Duration::from_secs(0))?;

    let runtime_result = monitor_plain_processes(&plan, &mut running, &shutdown);
    let stop_reason = determine_stop_reason(&runtime_result, &shutdown, "ctrl_c");
    emit_runtime_event(
        &plan.project_root,
        "instance_stopping",
        None,
        None,
        None,
        format_detail_fields(&[("reason", stop_reason.to_owned())]),
    );
    let shutdown_result = shutdown_running_services(
        &mut running,
        &shutdown,
        &plan,
        stop_reason,
    );

    let cleanup_result =
        runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());
    emit_runtime_event(
        &plan.project_root,
        "instance_stopped",
        None,
        None,
        None,
        format_detail_fields(&[
            ("runtime_ok", runtime_result.is_ok().to_string()),
            ("shutdown_ok", shutdown_result.is_ok().to_string()),
            ("cleanup_ok", cleanup_result.is_ok().to_string()),
        ]),
    );

    println!();
    runtime_result?;
    shutdown_result?;
    cleanup_result?;

    Ok(())
}

fn run_up_tui(plan: RunPlan) -> AppResult<()> {
    let lock = RuntimeLock::acquire(&plan.project_root)?;
    let (lock, _instance_log_guard) = install_plan_instance_logger(&plan, lock)?;
    let mut runtime_state = RuntimeState::new(
        plan.project_root.clone(),
        plan.config_path.clone(),
        plan.instance_log.as_ref().map(|log| log.file.clone()),
    );
    runtime_state::write_state(&plan.project_root, &runtime_state)?;
    emit_runtime_event(
        &plan.project_root,
        "instance_started",
        None,
        None,
        None,
        format_detail_fields(&[
            ("mode", "tui".to_owned()),
            ("config", plan.config_path.display().to_string()),
            ("service_count", plan.services.len().to_string()),
        ]),
    );

    let shutdown = install_shutdown_controller()?;

    let (log_tx, log_rx) = mpsc::channel();
    let mut running = match start_services(&plan, &mut runtime_state, |service| {
        OutputMode::Capture(CaptureOptions {
            event_sender: Some(log_tx.clone()),
            log: service.log.clone(),
            echo_to_terminal: false,
        })
    }) {
        Ok(running) => running,
        Err(error) => {
            let cleanup_result =
                runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());
            cleanup_result?;
            return Err(error);
        }
    };
    runtime_state::register_instance(&runtime_state)?;
    drop(log_tx);

    let runtime_result =
        crate::tui::run_dashboard(&plan.project_root, &mut running, log_rx, shutdown.clone());
    let stop_reason = determine_stop_reason(&runtime_result, &shutdown, "ctrl_c");
    emit_runtime_event(
        &plan.project_root,
        "instance_stopping",
        None,
        None,
        None,
        format_detail_fields(&[("reason", stop_reason.to_owned())]),
    );
    let shutdown_result = shutdown_running_services(
        &mut running,
        &shutdown,
        &plan,
        stop_reason,
    );
    let cleanup_result =
        runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());
    emit_runtime_event(
        &plan.project_root,
        "instance_stopped",
        None,
        None,
        None,
        format_detail_fields(&[
            ("runtime_ok", runtime_result.is_ok().to_string()),
            ("shutdown_ok", shutdown_result.is_ok().to_string()),
            ("cleanup_ok", cleanup_result.is_ok().to_string()),
        ]),
    );

    runtime_result?;
    shutdown_result?;
    cleanup_result?;

    Ok(())
}

fn run_up_daemonized(plan: RunPlan) -> AppResult<()> {
    let lock = RuntimeLock::acquire(&plan.project_root)?;
    let (lock, _instance_log_guard) = install_plan_instance_logger(&plan, lock)?;
    let mut runtime_state = RuntimeState::new(
        plan.project_root.clone(),
        plan.config_path.clone(),
        plan.instance_log.as_ref().map(|log| log.file.clone()),
    );
    runtime_state::write_state(&plan.project_root, &runtime_state)?;
    emit_runtime_event(
        &plan.project_root,
        "instance_started",
        None,
        None,
        None,
        format_detail_fields(&[
            ("mode", "daemon".to_owned()),
            ("config", plan.config_path.display().to_string()),
            ("service_count", plan.services.len().to_string()),
        ]),
    );

    let shutdown = install_shutdown_controller()?;
    let mut running = match start_services(&plan, &mut runtime_state, |service| {
        if service.log.is_some() {
            OutputMode::Capture(CaptureOptions {
                event_sender: None,
                log: service.log.clone(),
                echo_to_terminal: false,
            })
        } else {
            OutputMode::Null
        }
    }) {
        Ok(running) => running,
        Err(error) => {
            let cleanup_result =
                runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());
            cleanup_result?;
            return Err(error);
        }
    };
    runtime_state::register_instance(&runtime_state)?;

    let runtime_result = monitor_daemon_processes(&plan, &mut running, &shutdown);
    let stop_reason = determine_stop_reason(&runtime_result, &shutdown, "ctrl_c");
    emit_runtime_event(
        &plan.project_root,
        "instance_stopping",
        None,
        None,
        None,
        format_detail_fields(&[("reason", stop_reason.to_owned())]),
    );
    let shutdown_result = shutdown_running_services(
        &mut running,
        &shutdown,
        &plan,
        stop_reason,
    );
    let cleanup_result =
        runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());
    emit_runtime_event(
        &plan.project_root,
        "instance_stopped",
        None,
        None,
        None,
        format_detail_fields(&[
            ("runtime_ok", runtime_result.is_ok().to_string()),
            ("shutdown_ok", shutdown_result.is_ok().to_string()),
            ("cleanup_ok", cleanup_result.is_ok().to_string()),
        ]),
    );

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
    let hook_bundle = load_hook_bundle_from_state(&state);
    for service in state.services.iter().rev() {
        println!("stopping {}", service.service_name);
        if let Err(error) = stop_recorded_service(service, force, hook_bundle.as_ref(), "down") {
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
        let events = runtime_state::load_events(&entry.project_root).unwrap_or_default();
        let last_event = summarize_last_event(&events);
        let service_summaries = summarize_service_events(&events, &service_names);

        if tool_alive || state_exists || lock_exists {
            active.push(ManagementEntry::from_registry_entry(
                entry,
                tool_alive,
                alive_services,
                service_count,
                service_names,
                state_exists || lock_exists,
                last_event,
                service_summaries,
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
            "- pid {} | status {} | uptime {} | last {} | root {} | config {} | services: {}",
            entry.tool_pid,
            entry.status_summary,
            entry.uptime,
            entry
                .last_event
                .as_ref()
                .map(|event| event.event_type.as_str())
                .unwrap_or("-"),
            entry.project_root.display(),
            entry.config_path.display(),
            services
        );
        if let Some(instance_log_file) = &entry.instance_log_file {
            println!("  instance_log: {}", instance_log_file.display());
        }
        if let Some(event) = &entry.last_event {
            println!(
                "  recent: service={} hook={} action={} detail={}",
                event.service_name.as_deref().unwrap_or("-"),
                event.hook_name.as_deref().unwrap_or("-"),
                event.action_name.as_deref().unwrap_or("-"),
                event.detail
            );
        }
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
    instance_log_file: Option<PathBuf>,
    service_names: Vec<String>,
    service_count: usize,
    alive_services: usize,
    started_at_unix_secs: u64,
    uptime_secs: u64,
    uptime: String,
    status_summary: String,
    last_event: Option<ManagementEventSummary>,
    service_summaries: Vec<ManagementServiceSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct ManagementEventSummary {
    event_type: String,
    service_name: Option<String>,
    hook_name: Option<String>,
    action_name: Option<String>,
    detail: String,
    timestamp_unix_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
struct ManagementServiceSummary {
    service_name: String,
    last_hook_name: Option<String>,
    last_hook_status: String,
    last_action_name: Option<String>,
    last_action_status: String,
    last_detail: Option<String>,
}

impl ManagementEntry {
    fn from_registry_entry(
        entry: RegistryEntry,
        tool_alive: bool,
        alive_services: usize,
        service_count: usize,
        service_names: Vec<String>,
        has_runtime_artifacts: bool,
        last_event: Option<ManagementEventSummary>,
        service_summaries: Vec<ManagementServiceSummary>,
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
            instance_log_file: entry.instance_log_file,
            service_names,
            service_count,
            alive_services,
            started_at_unix_secs: entry.started_at_unix_secs,
            uptime_secs,
            uptime: format_elapsed(Duration::from_secs(uptime_secs)),
            status_summary,
            last_event,
            service_summaries,
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

fn summarize_last_event(events: &[RuntimeEvent]) -> Option<ManagementEventSummary> {
    events.last().map(|event| ManagementEventSummary {
        event_type: event.event_type.clone(),
        service_name: event.service_name.clone(),
        hook_name: event.hook_name.clone(),
        action_name: event.action_name.clone(),
        detail: event.detail.clone(),
        timestamp_unix_secs: event.timestamp_unix_secs,
    })
}

fn summarize_service_events(
    events: &[RuntimeEvent],
    service_names: &[String],
) -> Vec<ManagementServiceSummary> {
    service_names
        .iter()
        .map(|service_name| {
            let mut summary = ManagementServiceSummary {
                service_name: service_name.clone(),
                last_hook_name: None,
                last_hook_status: "unknown".to_owned(),
                last_action_name: None,
                last_action_status: "unknown".to_owned(),
                last_detail: None,
            };

            for event in events
                .iter()
                .filter(|event| event.service_name.as_deref() == Some(service_name.as_str()))
            {
                match event.event_type.as_str() {
                    "hook_started" => {
                        summary.last_hook_name = event.hook_name.clone();
                        summary.last_hook_status = "running".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    "hook_finished" => {
                        summary.last_hook_name = event.hook_name.clone();
                        summary.last_hook_status = "finished".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    "hook_failed" => {
                        summary.last_hook_name = event.hook_name.clone();
                        summary.last_hook_status = "failed".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    "action_started" => {
                        summary.last_action_name = event.action_name.clone();
                        summary.last_action_status = "running".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    "action_finished" => {
                        summary.last_action_name = event.action_name.clone();
                        summary.last_action_status = "finished".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    "action_failed" => {
                        summary.last_action_name = event.action_name.clone();
                        summary.last_action_status = "failed".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    "action_timed_out" => {
                        summary.last_action_name = event.action_name.clone();
                        summary.last_action_status = "timeout".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    "service_stop_timeout" => {
                        summary.last_hook_name =
                            Some(HookName::AfterStopTimeout.as_str().to_owned());
                        summary.last_hook_status = "timeout".to_owned();
                        summary.last_detail = Some(event.detail.clone());
                    }
                    _ => {}
                }
            }

            summary
        })
        .collect()
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time must be after unix epoch")
        .as_secs()
}

fn monitor_plain_processes(
    plan: &RunPlan,
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
                if !runtime_state::state_path(&plan.project_root).exists() {
                    return Ok(());
                }
                handle_runtime_exit(plan, process, status)?;
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

fn start_services<F>(
    plan: &RunPlan,
    runtime_state: &mut RuntimeState,
    mut output_mode_for: F,
) -> AppResult<Vec<SpawnedProcess>>
where
    F: FnMut(&ResolvedServiceConfig) -> OutputMode,
{
    let mut running = Vec::new();
    for service in &plan.services {
        emit_runtime_event(
            &plan.project_root,
            "service_starting",
            Some(&service.name),
            None,
            None,
            format!("starting service `{}`", service.name),
        );
        if let Err(error) = run_hook_with_context(
            plan,
            service,
            HookName::BeforeStart,
            HookRuntimeExtras {
                service_pid: None,
                stop_reason: None,
                exit_code: None,
                exit_status: None,
            },
        ) {
            emit_runtime_event(
                &plan.project_root,
                "service_start_aborted",
                Some(&service.name),
                Some(HookName::BeforeStart.as_str()),
                None,
                error.to_string(),
            );
            shutdown_running_services(
                &mut running,
                &ShutdownController::force_requested_now(),
                plan,
                "startup_failure",
            )?;
            return Err(error);
        }

        let spawned = match process::spawn_service(service, output_mode_for(service)) {
            Ok(spawned) => spawned,
            Err(error) => {
                emit_runtime_event(
                    &plan.project_root,
                    "service_spawn_failed",
                    Some(&service.name),
                    None,
                    None,
                    error.to_string(),
                );
                if let Err(hook_error) = run_hook_with_context(
                    plan,
                    service,
                    HookName::AfterStartFailure,
                    HookRuntimeExtras {
                        service_pid: None,
                        stop_reason: None,
                        exit_code: None,
                        exit_status: Some(error.to_string()),
                    },
                ) {
                    eprintln!("{hook_error}");
                }
                shutdown_running_services(
                    &mut running,
                    &ShutdownController::force_requested_now(),
                    plan,
                    "startup_failure",
                )?;
                return Err(error);
            }
        };
        runtime_state.services.push(spawned.state.clone());
        runtime_state::write_state(&plan.project_root, runtime_state)?;
        emit_runtime_event(
            &plan.project_root,
            "service_running",
            Some(&service.name),
            None,
            None,
            format!("service `{}` entered running with pid {}", service.name, spawned.state.pid),
        );
        if let Err(error) = run_hook_with_context(
            plan,
            service,
            HookName::AfterStartSuccess,
            HookRuntimeExtras {
                service_pid: Some(spawned.state.pid),
                stop_reason: None,
                exit_code: Some(0),
                exit_status: Some("running".to_owned()),
            },
        ) {
            eprintln!("{error}");
        }
        running.push(spawned);
    }
    Ok(running)
}

fn handle_runtime_exit(
    plan: &RunPlan,
    process: &mut SpawnedProcess,
    status: std::process::ExitStatus,
) -> AppResult<()> {
    emit_runtime_event(
        &plan.project_root,
        "service_runtime_exit_unexpected",
        Some(&process.state.service_name),
        Some(HookName::AfterRuntimeExitUnexpected.as_str()),
        None,
        format!(
            "service `{}` exited unexpectedly with status {status}",
            process.state.service_name
        ),
    );

    let Some(service) = plan
        .services
        .iter()
        .find(|service| service.name == process.state.service_name)
    else {
        return Ok(());
    };

    if let Err(error) = run_hook_with_context(
        plan,
        service,
        HookName::AfterRuntimeExitUnexpected,
        HookRuntimeExtras {
            service_pid: Some(process.state.pid),
            stop_reason: None,
            exit_code: status.code(),
            exit_status: Some(status.to_string()),
        },
    ) {
        eprintln!("{error}");
    }

    Ok(())
}

fn monitor_daemon_processes(
    plan: &RunPlan,
    running: &mut [SpawnedProcess],
    shutdown: &ShutdownController,
) -> AppResult<()> {
    loop {
        if shutdown.shutdown_requested() {
            return Ok(());
        }

        for process in running.iter_mut() {
            if let Some(status) = process::service_exited(&mut process.child)? {
                if !runtime_state::state_path(&plan.project_root).exists() {
                    return Ok(());
                }
                handle_runtime_exit(plan, process, status)?;
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
    plan: &RunPlan,
    stop_reason: &str,
) -> AppResult<()> {
    let mut first_error = None;
    let mut force_notice_shown = false;
    for process in running.iter_mut().rev() {
        if let Err(error) = stop_spawned_process(
            process,
            shutdown,
            &mut force_notice_shown,
            plan,
            stop_reason,
        ) {
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
    plan: &RunPlan,
    stop_reason: &str,
) -> AppResult<()> {
    if process::service_exited(&mut process.child)?.is_some() {
        return Ok(());
    }

    let Some(service_config) = plan
        .services
        .iter()
        .find(|service| service.name == process.state.service_name)
    else {
        return Err(AppError::runtime_failed(format!(
            "service config for `{}` not found during shutdown",
            process.state.service_name
        )));
    };

    emit_runtime_event(
        &plan.project_root,
        "service_stopping",
        Some(&process.state.service_name),
        None,
        None,
        format!(
            "stopping service `{}` with reason `{stop_reason}`",
            process.state.service_name
        ),
    );

    if let Err(error) = run_hook_with_context(
        plan,
        service_config,
        HookName::BeforeStop,
        HookRuntimeExtras {
            service_pid: Some(process.state.pid),
            stop_reason: Some(stop_reason.to_owned()),
            exit_code: None,
            exit_status: None,
        },
    ) {
        eprintln!("{error}");
    }

    if shutdown.force_requested() {
        if !*force_notice_shown {
            eprintln!("second interrupt received, force-stopping services.");
            *force_notice_shown = true;
        }
        force_kill_process(process)?;
        let _ = process.child.wait();
        emit_runtime_event(
            &plan.project_root,
            "service_stopped",
            Some(&process.state.service_name),
            None,
            None,
            format!(
                "service `{}` stopped via force request",
                process.state.service_name
            ),
        );
        if let Err(error) = run_hook_with_context(
            plan,
            service_config,
            HookName::AfterStopSuccess,
            HookRuntimeExtras {
                service_pid: Some(process.state.pid),
                stop_reason: Some(stop_reason.to_owned()),
                exit_code: None,
                exit_status: Some("forced".to_owned()),
            },
        ) {
            eprintln!("{error}");
        }
        return Ok(());
    }

    if let Err(error) = process::request_stop_service(&process.state) {
        let final_error = fallback_kill_process(process, Some(error));
        if let Err(hook_error) = run_hook_with_context(
            plan,
            service_config,
            HookName::AfterStopFailure,
            HookRuntimeExtras {
                service_pid: Some(process.state.pid),
                stop_reason: Some(stop_reason.to_owned()),
                exit_code: None,
                exit_status: Some("request_stop_failed".to_owned()),
            },
        ) {
            eprintln!("{hook_error}");
        }
        return final_error;
    }

    let deadline = Instant::now() + Duration::from_secs(process.state.stop_timeout_secs);
    loop {
        if process::service_exited(&mut process.child)?.is_some() {
            emit_runtime_event(
                &plan.project_root,
                "service_stopped",
                Some(&process.state.service_name),
                None,
                None,
                format!(
                    "service `{}` stopped gracefully",
                    process.state.service_name
                ),
            );
            if let Err(error) = run_hook_with_context(
                plan,
                service_config,
                HookName::AfterStopSuccess,
                HookRuntimeExtras {
                    service_pid: Some(process.state.pid),
                    stop_reason: Some(stop_reason.to_owned()),
                    exit_code: Some(0),
                    exit_status: Some("graceful".to_owned()),
                },
            ) {
                eprintln!("{error}");
            }
            return Ok(());
        }

        if shutdown.force_requested() {
            if !*force_notice_shown {
                eprintln!("second interrupt received, force-stopping services.");
                *force_notice_shown = true;
            }
            force_kill_process(process)?;
            let _ = process.child.wait();
            emit_runtime_event(
                &plan.project_root,
                "service_stopped",
                Some(&process.state.service_name),
                None,
                None,
                format!(
                    "service `{}` stopped via force request",
                    process.state.service_name
                ),
            );
            if let Err(error) = run_hook_with_context(
                plan,
                service_config,
                HookName::AfterStopSuccess,
                HookRuntimeExtras {
                    service_pid: Some(process.state.pid),
                    stop_reason: Some(stop_reason.to_owned()),
                    exit_code: None,
                    exit_status: Some("forced".to_owned()),
                },
            ) {
                eprintln!("{error}");
            }
            return Ok(());
        }

        if Instant::now() >= deadline {
            emit_runtime_event(
                &plan.project_root,
                "service_stop_timeout",
                Some(&process.state.service_name),
                Some(HookName::AfterStopTimeout.as_str()),
                None,
                format!(
                    "service `{}` reached stop timeout",
                    process.state.service_name
                ),
            );
            if let Err(error) = run_hook_with_context(
                plan,
                service_config,
                HookName::AfterStopTimeout,
                HookRuntimeExtras {
                    service_pid: Some(process.state.pid),
                    stop_reason: Some(stop_reason.to_owned()),
                    exit_code: None,
                    exit_status: Some("timeout".to_owned()),
                },
            ) {
                eprintln!("{error}");
            }

            let force_result = force_kill_process(process);
            let _ = process.child.wait();
            match force_result {
                Ok(()) => {
                    emit_runtime_event(
                        &plan.project_root,
                        "service_stopped",
                        Some(&process.state.service_name),
                        None,
                        None,
                        format!(
                            "service `{}` stopped after timeout escalation",
                            process.state.service_name
                        ),
                    );
                    if let Err(error) = run_hook_with_context(
                        plan,
                        service_config,
                        HookName::AfterStopSuccess,
                        HookRuntimeExtras {
                            service_pid: Some(process.state.pid),
                            stop_reason: Some(stop_reason.to_owned()),
                            exit_code: None,
                            exit_status: Some("timed_out_then_forced".to_owned()),
                        },
                    ) {
                        eprintln!("{error}");
                    }
                    return Ok(());
                }
                Err(error) => {
                    emit_runtime_event(
                        &plan.project_root,
                        "service_stop_failed",
                        Some(&process.state.service_name),
                        Some(HookName::AfterStopFailure.as_str()),
                        None,
                        error.to_string(),
                    );
                    if let Err(hook_error) = run_hook_with_context(
                        plan,
                        service_config,
                        HookName::AfterStopFailure,
                        HookRuntimeExtras {
                            service_pid: Some(process.state.pid),
                            stop_reason: Some(stop_reason.to_owned()),
                            exit_code: None,
                            exit_status: Some("force_stop_failed".to_owned()),
                        },
                    ) {
                        eprintln!("{hook_error}");
                    }
                    return Err(error);
                }
            }
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

fn determine_stop_reason(
    runtime_result: &AppResult<()>,
    shutdown: &ShutdownController,
    signal_reason: &'static str,
) -> &'static str {
    if runtime_result.is_err() {
        "runtime_failure"
    } else if shutdown.shutdown_requested() {
        signal_reason
    } else {
        "shutdown"
    }
}

fn emit_runtime_event(
    project_root: &Path,
    event_type: &str,
    service_name: Option<&str>,
    hook_name: Option<&str>,
    action_name: Option<&str>,
    detail: String,
) {
    let event = RuntimeEvent {
        timestamp_unix_secs: current_unix_secs(),
        event_type: event_type.to_owned(),
        service_name: service_name.map(str::to_owned),
        hook_name: hook_name.map(str::to_owned),
        action_name: action_name.map(str::to_owned),
        detail,
    };
    let _ = runtime_state::append_event(project_root, &event);
    INSTANCE_LOGGER.with(|logger| {
        let logger = logger.borrow();
        let Some(logger) = logger.as_ref() else {
            return;
        };
        if let Ok(mut logger) = logger.lock() {
            let _ = logger.write_line(&format_instance_log_line(&event));
        }
    });
}

fn format_instance_log_line(event: &RuntimeEvent) -> String {
    let mut fields = vec![
        format!("ts={}", event.timestamp_unix_secs),
        format!("level={}", event_level(event.event_type.as_str())),
        format!("event={}", quote_log_value(&event.event_type)),
    ];
    append_log_field(&mut fields, "service", event.service_name.as_deref());
    append_log_field(&mut fields, "hook", event.hook_name.as_deref());
    append_log_field(&mut fields, "action", event.action_name.as_deref());
    append_log_field(&mut fields, "detail", Some(event.detail.as_str()));
    fields.join(" ")
}

fn event_level(event_type: &str) -> &'static str {
    match event_type {
        "service_spawn_failed"
        | "service_start_failed"
        | "service_stop_failed"
        | "hook_failed"
        | "action_failed"
        | "service_runtime_exit_unexpected" => "ERROR",
        "instance_stopping" | "service_stop_timeout" | "action_timed_out" => "WARN",
        _ => "INFO",
    }
}

fn append_log_field(fields: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        fields.push(format!("{key}={}", quote_log_value(value)));
    }
}

fn format_detail_fields(fields: &[(&str, String)]) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{key}={}", quote_log_value(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_log_value(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

#[derive(Clone)]
struct HookBundle {
    project_root: PathBuf,
    config_path: PathBuf,
    actions: BTreeMap<String, ResolvedActionConfig>,
    services: BTreeMap<String, ResolvedServiceConfig>,
}

#[derive(Clone)]
struct HookRuntimeExtras {
    service_pid: Option<u32>,
    stop_reason: Option<String>,
    exit_code: Option<i32>,
    exit_status: Option<String>,
}

fn load_hook_bundle_from_state(state: &runtime_state::RuntimeState) -> Option<HookBundle> {
    let config = ProjectConfig::load(&state.config_path).ok()?;
    let mut services = BTreeMap::new();
    for service_state in &state.services {
        let resolved = config
            .resolve_service(&service_state.service_name, &state.project_root)
            .ok()?;
        services.insert(service_state.service_name.clone(), resolved);
    }

    Some(HookBundle {
        project_root: state.project_root.clone(),
        config_path: state.config_path.clone(),
        actions: config.resolve_actions(&state.project_root).ok()?,
        services,
    })
}

fn run_hook_with_context(
    plan: &RunPlan,
    service: &ResolvedServiceConfig,
    hook: HookName,
    extras: HookRuntimeExtras,
) -> AppResult<()> {
    let action_names = service.hooks.actions_for(hook);
    if action_names.is_empty() {
        return Ok(());
    }

    emit_runtime_event(
        &plan.project_root,
        "hook_started",
        Some(&service.name),
        Some(hook.as_str()),
        None,
        format!(
            "hook `{}` started for service `{}` with {} action(s)",
            hook.as_str(),
            service.name,
            action_names.len()
        ),
    );

    for action_name in action_names {
        let action = plan.actions.get(action_name).ok_or_else(|| {
            AppError::config_invalid(format!(
                "service `{}` references unknown action `{action_name}` in `{}`",
                service.name,
                hook.as_str()
            ))
        })?;
        emit_runtime_event(
            &plan.project_root,
            "action_started",
            Some(&service.name),
            Some(hook.as_str()),
            Some(&action.name),
            format!(
                "action `{}` started for service `{}` hook `{}`",
                action.name,
                service.name,
                hook.as_str()
            ),
        );

        let context = ActionRenderContext {
            project_root: plan.project_root.clone(),
            config_path: plan.config_path.clone(),
            service_name: service.name.clone(),
            action_name: action.name.clone(),
            hook_name: hook,
            service_cwd: service.cwd.clone(),
            service_executable: service.executable.clone(),
            service_pid: extras.service_pid,
            stop_reason: extras.stop_reason.clone(),
            exit_code: extras.exit_code,
            exit_status: extras.exit_status.clone(),
        };
        let mut rendered_action = action.clone();
        rendered_action.args = context.render_args(&action.args)?;
        match process::run_action(&rendered_action, &service.name, hook)? {
            ActionRunStatus::Succeeded => {
                emit_runtime_event(
                    &plan.project_root,
                    "action_finished",
                    Some(&service.name),
                    Some(hook.as_str()),
                    Some(&action.name),
                    format!(
                        "action `{}` finished for service `{}` hook `{}`",
                        action.name,
                        service.name,
                        hook.as_str()
                    ),
                );
            }
            ActionRunStatus::Failed { status } => {
                let error = AppError::startup_failed(format!(
                    "action `{}` for service `{}` hook `{}` exited with status {status}",
                    action.name,
                    service.name,
                    hook.as_str()
                ));
                emit_runtime_event(
                    &plan.project_root,
                    "action_failed",
                    Some(&service.name),
                    Some(hook.as_str()),
                    Some(&action.name),
                    error.to_string(),
                );
                emit_runtime_event(
                    &plan.project_root,
                    "hook_failed",
                    Some(&service.name),
                    Some(hook.as_str()),
                    None,
                    error.to_string(),
                );
                return Err(error);
            }
            ActionRunStatus::TimedOut { timeout_secs } => {
                let error = AppError::startup_failed(format!(
                    "action `{}` for service `{}` hook `{}` timed out after {} seconds",
                    action.name,
                    service.name,
                    hook.as_str(),
                    timeout_secs
                ));
                emit_runtime_event(
                    &plan.project_root,
                    "action_timed_out",
                    Some(&service.name),
                    Some(hook.as_str()),
                    Some(&action.name),
                    error.to_string(),
                );
                emit_runtime_event(
                    &plan.project_root,
                    "hook_failed",
                    Some(&service.name),
                    Some(hook.as_str()),
                    None,
                    error.to_string(),
                );
                return Err(error);
            }
        }
    }

    emit_runtime_event(
        &plan.project_root,
        "hook_finished",
        Some(&service.name),
        Some(hook.as_str()),
        None,
        format!("hook `{}` finished for service `{}`", hook.as_str(), service.name),
    );
    Ok(())
}

fn run_hook_with_bundle(
    bundle: &HookBundle,
    service: &ResolvedServiceConfig,
    hook: HookName,
    extras: HookRuntimeExtras,
) -> AppResult<()> {
    let action_names = service.hooks.actions_for(hook);
    if action_names.is_empty() {
        return Ok(());
    }

    emit_runtime_event(
        &bundle.project_root,
        "hook_started",
        Some(&service.name),
        Some(hook.as_str()),
        None,
        format!(
            "hook `{}` started for service `{}` with {} action(s)",
            hook.as_str(),
            service.name,
            action_names.len()
        ),
    );

    for action_name in action_names {
        let action = bundle.actions.get(action_name).ok_or_else(|| {
            AppError::config_invalid(format!(
                "service `{}` references unknown action `{action_name}` in `{}`",
                service.name,
                hook.as_str()
            ))
        })?;
        emit_runtime_event(
            &bundle.project_root,
            "action_started",
            Some(&service.name),
            Some(hook.as_str()),
            Some(&action.name),
            format!(
                "action `{}` started for service `{}` hook `{}`",
                action.name,
                service.name,
                hook.as_str()
            ),
        );

        let context = ActionRenderContext {
            project_root: bundle.project_root.clone(),
            config_path: bundle.config_path.clone(),
            service_name: service.name.clone(),
            action_name: action.name.clone(),
            hook_name: hook,
            service_cwd: service.cwd.clone(),
            service_executable: service.executable.clone(),
            service_pid: extras.service_pid,
            stop_reason: extras.stop_reason.clone(),
            exit_code: extras.exit_code,
            exit_status: extras.exit_status.clone(),
        };
        let mut rendered_action = action.clone();
        rendered_action.args = context.render_args(&action.args)?;
        match process::run_action(&rendered_action, &service.name, hook)? {
            ActionRunStatus::Succeeded => {
                emit_runtime_event(
                    &bundle.project_root,
                    "action_finished",
                    Some(&service.name),
                    Some(hook.as_str()),
                    Some(&action.name),
                    format!(
                        "action `{}` finished for service `{}` hook `{}`",
                        action.name,
                        service.name,
                        hook.as_str()
                    ),
                );
            }
            ActionRunStatus::Failed { status } => {
                let error = AppError::startup_failed(format!(
                    "action `{}` for service `{}` hook `{}` exited with status {status}",
                    action.name,
                    service.name,
                    hook.as_str()
                ));
                emit_runtime_event(
                    &bundle.project_root,
                    "action_failed",
                    Some(&service.name),
                    Some(hook.as_str()),
                    Some(&action.name),
                    error.to_string(),
                );
                emit_runtime_event(
                    &bundle.project_root,
                    "hook_failed",
                    Some(&service.name),
                    Some(hook.as_str()),
                    None,
                    error.to_string(),
                );
                return Err(error);
            }
            ActionRunStatus::TimedOut { timeout_secs } => {
                let error = AppError::startup_failed(format!(
                    "action `{}` for service `{}` hook `{}` timed out after {} seconds",
                    action.name,
                    service.name,
                    hook.as_str(),
                    timeout_secs
                ));
                emit_runtime_event(
                    &bundle.project_root,
                    "action_timed_out",
                    Some(&service.name),
                    Some(hook.as_str()),
                    Some(&action.name),
                    error.to_string(),
                );
                emit_runtime_event(
                    &bundle.project_root,
                    "hook_failed",
                    Some(&service.name),
                    Some(hook.as_str()),
                    None,
                    error.to_string(),
                );
                return Err(error);
            }
        }
    }

    emit_runtime_event(
        &bundle.project_root,
        "hook_finished",
        Some(&service.name),
        Some(hook.as_str()),
        None,
        format!("hook `{}` finished for service `{}`", hook.as_str(), service.name),
    );
    Ok(())
}

fn stop_recorded_service(
    service: &runtime_state::ServiceRuntimeState,
    force: bool,
    hook_bundle: Option<&HookBundle>,
    stop_reason: &'static str,
) -> AppResult<()> {
    if !process::is_pid_alive(service.pid) {
        return Ok(());
    }

    if let Some(bundle) = hook_bundle {
        emit_runtime_event(
            &bundle.project_root,
            "service_stopping",
            Some(&service.service_name),
            None,
            None,
            format!(
                "stopping recorded service `{}` with reason `{stop_reason}`",
                service.service_name
            ),
        );
    }

    if let Some(bundle) = hook_bundle
        && let Some(service_config) = bundle.services.get(&service.service_name)
        && let Err(error) = run_hook_with_bundle(
            bundle,
            service_config,
            HookName::BeforeStop,
            HookRuntimeExtras {
                service_pid: Some(service.pid),
                stop_reason: Some(stop_reason.to_owned()),
                exit_code: None,
                exit_status: None,
            },
        )
    {
        eprintln!("{error}");
    }

    let outcome = match process::stop_service_with_outcome(service, force) {
        Ok(outcome) => outcome,
        Err(error) => {
            if let Some(bundle) = hook_bundle {
                emit_runtime_event(
                    &bundle.project_root,
                    "service_stop_failed",
                    Some(&service.service_name),
                    Some(HookName::AfterStopFailure.as_str()),
                    None,
                    error.to_string(),
                );
            }
            if let Some(bundle) = hook_bundle
                && let Some(service_config) = bundle.services.get(&service.service_name)
                && let Err(hook_error) = run_hook_with_bundle(
                    bundle,
                    service_config,
                    HookName::AfterStopFailure,
                    HookRuntimeExtras {
                        service_pid: Some(service.pid),
                        stop_reason: Some(stop_reason.to_owned()),
                        exit_code: None,
                        exit_status: Some(error.to_string()),
                    },
                )
            {
                eprintln!("{hook_error}");
            }
            return Err(error);
        }
    };

    if let Some(bundle) = hook_bundle
        && let Some(service_config) = bundle.services.get(&service.service_name)
    {
        if outcome == StopOutcome::TimedOutForced
            && {
                emit_runtime_event(
                    &bundle.project_root,
                    "service_stop_timeout",
                    Some(&service.service_name),
                    Some(HookName::AfterStopTimeout.as_str()),
                    None,
                    format!("service `{}` reached stop timeout", service.service_name),
                );
                true
            }
            && let Err(error) = run_hook_with_bundle(
                bundle,
                service_config,
                HookName::AfterStopTimeout,
                HookRuntimeExtras {
                    service_pid: Some(service.pid),
                    stop_reason: Some(stop_reason.to_owned()),
                    exit_code: None,
                    exit_status: Some("timeout".to_owned()),
                },
            )
        {
            eprintln!("{error}");
        }

        if let Err(error) = run_hook_with_bundle(
            bundle,
            service_config,
            HookName::AfterStopSuccess,
            HookRuntimeExtras {
                service_pid: Some(service.pid),
                stop_reason: Some(stop_reason.to_owned()),
                exit_code: Some(0),
                exit_status: Some(match outcome {
                    StopOutcome::Graceful => "graceful".to_owned(),
                    StopOutcome::TimedOutForced => "timed_out_then_forced".to_owned(),
                    StopOutcome::Forced => "forced".to_owned(),
                }),
            },
        ) {
            eprintln!("{error}");
        }

        emit_runtime_event(
            &bundle.project_root,
            "service_stopped",
            Some(&service.service_name),
            None,
            None,
            format!(
                "recorded service `{}` stopped with outcome `{}`",
                service.service_name,
                match outcome {
                    StopOutcome::Graceful => "graceful",
                    StopOutcome::TimedOutForced => "timed_out_then_forced",
                    StopOutcome::Forced => "forced",
                }
            ),
        );
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
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::config::{HookName, ProjectConfig, ResolvedLogConfig};
    use crate::process::SpawnedProcess;
    use crate::runtime_state::{
        self, PlatformRuntimeState, RegistryEntry, RuntimeEvent, ServiceRuntimeState,
    };

    use super::{
        build_run_plan, current_unix_secs, emit_runtime_event, format_detail_fields,
        handle_runtime_exit, install_instance_logger, run_hook_with_context, run_init,
        summarize_instance_status, summarize_last_event, summarize_service_events,
        HookRuntimeExtras, ManagementEntry,
    };

    #[test]
    fn init_writes_template_and_refuses_overwrite() {
        let dir = temp_dir("init-template");
        let path = dir.join("onekey-tasks.yaml");

        run_init(&path, false).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        let parsed = ProjectConfig::load(&path).unwrap();
        assert_eq!(parsed.actions.len(), 0);
        assert_eq!(parsed.services.len(), 2);
        assert!(raw.contains("onekey-run.log"));
        assert!(raw.contains("services:"));
        assert!(raw.contains("depends_on"));
        assert!(raw.contains("log:"));
        assert!(!raw.contains("actions:"));
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
        let parsed = ProjectConfig::load(&path).unwrap();
        assert_eq!(parsed.actions.len(), 4);
        assert_eq!(parsed.services.len(), 2);
        assert!(raw.contains("actions:"));
        assert!(raw.contains("onekey-run.log"));
        assert!(raw.contains("prepare-app:"));
        assert!(raw.contains("after_start_success:"));
        assert!(raw.contains("before_stop:"));
        assert!(raw.contains("after_runtime_exit_unexpected:"));
        assert!(raw.contains("notify-exit:"));
        assert!(raw.contains("restart:"));
        assert!(raw.contains("env:"));
        assert!(raw.contains("./logs/app.log"));
        assert!(raw.contains("stop_signal:"));
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

    #[test]
    fn summarizes_last_event_and_service_events() {
        let events = vec![
            RuntimeEvent {
                timestamp_unix_secs: 1,
                event_type: "hook_started".to_owned(),
                service_name: Some("api".to_owned()),
                hook_name: Some("before_start".to_owned()),
                action_name: None,
                detail: "started".to_owned(),
            },
            RuntimeEvent {
                timestamp_unix_secs: 2,
                event_type: "action_failed".to_owned(),
                service_name: Some("api".to_owned()),
                hook_name: Some("before_start".to_owned()),
                action_name: Some("prepare".to_owned()),
                detail: "exit 1".to_owned(),
            },
        ];

        let last = summarize_last_event(&events).unwrap();
        assert_eq!(last.event_type, "action_failed");

        let summaries = summarize_service_events(&events, &["api".to_owned()]);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].last_action_status, "failed");
        assert_eq!(summaries[0].last_hook_status, "running");
    }

    #[test]
    fn management_entry_carries_instance_log_path() {
        let entry = ManagementEntry::from_registry_entry(
            RegistryEntry {
                project_root: PathBuf::from("/tmp/project"),
                config_path: PathBuf::from("/tmp/project/onekey-tasks.yaml"),
                instance_log_file: Some(PathBuf::from("/tmp/project/logs/onekey-run.log")),
                tool_pid: 42,
                started_at_unix_secs: current_unix_secs(),
                service_names: vec!["api".to_owned()],
            },
            true,
            1,
            1,
            vec!["api".to_owned()],
            true,
            None,
            Vec::new(),
        );

        assert_eq!(
            entry.instance_log_file,
            Some(PathBuf::from("/tmp/project/logs/onekey-run.log"))
        );
    }

    #[test]
    fn instance_detail_format_is_stable_key_value() {
        let detail = format_detail_fields(&[
            ("mode", "daemon".to_owned()),
            ("config", "/tmp/project/onekey-tasks.yaml".to_owned()),
            ("service_count", "2".to_owned()),
        ]);

        assert_eq!(
            detail,
            "mode=\"daemon\" config=\"/tmp/project/onekey-tasks.yaml\" service_count=\"2\""
        );
    }

    #[test]
    fn runtime_events_are_written_to_instance_log_when_enabled() {
        let dir = temp_dir("instance-log");
        let log_path = dir.join("logs").join("onekey-run.log");
        let _guard = install_instance_logger(Some(ResolvedLogConfig {
            file: log_path.clone(),
            append: true,
            max_file_bytes: None,
            overflow_strategy: None,
            rotate_file_count: None,
        }))
        .unwrap();

        emit_runtime_event(
            &dir,
            "hook_started",
            Some("api"),
            Some("before_start"),
            None,
            "hook `before_start` started".to_owned(),
        );

        let raw = fs::read_to_string(&log_path).unwrap();
        assert!(raw.contains("level=INFO"));
        assert!(raw.contains("event=\"hook_started\""));
        assert!(raw.contains("service=\"api\""));
        assert!(raw.contains("hook=\"before_start\""));

        drop(_guard);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn before_start_actions_run_successfully() {
        let dir = temp_dir("before-start-ok");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, success_action_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let result = run_hook_with_context(
            &plan,
            &plan.services[0],
            HookName::BeforeStart,
            HookRuntimeExtras {
                service_pid: None,
                stop_reason: None,
                exit_code: None,
                exit_status: None,
            },
        );

        assert!(result.is_ok(), "{result:?}");
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"hook_started\""));
        assert!(events_raw.contains("\"event_type\":\"action_finished\""));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn before_start_action_failure_aborts_startup() {
        let dir = temp_dir("before-start-fail");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, failing_action_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let error = run_hook_with_context(
            &plan,
            &plan.services[0],
            HookName::BeforeStart,
            HookRuntimeExtras {
                service_pid: None,
                stop_reason: None,
                exit_code: None,
                exit_status: None,
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("action `prepare`"));
        assert!(error.to_string().contains("hook `before_start`"));
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"action_failed\""));
        assert!(events_raw.contains("\"event_type\":\"hook_failed\""));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn before_start_action_timeout_emits_timeout_event() {
        let dir = temp_dir("before-start-timeout");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, timeout_action_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let error = run_hook_with_context(
            &plan,
            &plan.services[0],
            HookName::BeforeStart,
            HookRuntimeExtras {
                service_pid: None,
                stop_reason: None,
                exit_code: None,
                exit_status: None,
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("timed out"));
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"action_timed_out\""));
        assert!(events_raw.contains("\"event_type\":\"hook_failed\""));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_exit_unexpected_runs_hook_and_emits_event() {
        let dir = temp_dir("runtime-exit");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, runtime_exit_action_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();

        let mut child = runtime_exit_child();
        let pid = child.id();
        let status = child.wait().unwrap();
        let mut spawned = SpawnedProcess {
            child,
            state: ServiceRuntimeState {
                service_name: "api".to_owned(),
                pid,
                cwd: dir.clone(),
                executable: runtime_exit_service_executable().to_owned(),
                args: runtime_exit_service_args(),
                log_file: None,
                stop_signal: None,
                stop_timeout_secs: 10,
                platform: PlatformRuntimeState::default(),
            },
        };

        handle_runtime_exit(&plan, &mut spawned, status).unwrap();

        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"service_runtime_exit_unexpected\""));
        assert!(events_raw.contains("\"hook_name\":\"after_runtime_exit_unexpected\""));

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    fn success_action_config() -> &'static str {
        r#"
actions:
  prepare:
    executable: "sh"
    args: ["-c", "exit 0"]
services:
  api:
    executable: "sh"
    args: ["-c", "sleep 5"]
    hooks:
      before_start: ["prepare"]
"#
    }

    #[cfg(windows)]
    fn success_action_config() -> &'static str {
        r#"
actions:
  prepare:
    executable: "cmd"
    args: ["/C", "exit 0"]
services:
  api:
    executable: "cmd"
    args: ["/C", "timeout /T 5 /NOBREAK >NUL"]
    hooks:
      before_start: ["prepare"]
"#
    }

    #[cfg(unix)]
    fn failing_action_config() -> &'static str {
        r#"
actions:
  prepare:
    executable: "sh"
    args: ["-c", "exit 1"]
services:
  api:
    executable: "sh"
    args: ["-c", "sleep 5"]
    hooks:
      before_start: ["prepare"]
"#
    }

    #[cfg(unix)]
    fn runtime_exit_action_config() -> &'static str {
        r#"
actions:
  on-exit:
    executable: "sh"
    args: ["-c", "exit 0"]
services:
  api:
    executable: "sh"
    args: ["-c", "sleep 5"]
    hooks:
      after_runtime_exit_unexpected: ["on-exit"]
"#
    }

    #[cfg(windows)]
    fn runtime_exit_action_config() -> &'static str {
        r#"
actions:
  on-exit:
    executable: "cmd"
    args: ["/C", "exit 0"]
services:
  api:
    executable: "cmd"
    args: ["/C", "timeout /T 5 /NOBREAK >NUL"]
    hooks:
      after_runtime_exit_unexpected: ["on-exit"]
"#
    }

    #[cfg(unix)]
    fn runtime_exit_child() -> std::process::Child {
        Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap()
    }

    #[cfg(windows)]
    fn runtime_exit_child() -> std::process::Child {
        Command::new("cmd").args(["/C", "exit 7"]).spawn().unwrap()
    }

    #[cfg(unix)]
    fn runtime_exit_service_executable() -> &'static str {
        "sh"
    }

    #[cfg(windows)]
    fn runtime_exit_service_executable() -> &'static str {
        "cmd"
    }

    #[cfg(unix)]
    fn runtime_exit_service_args() -> Vec<String> {
        vec!["-c".to_owned(), "sleep 5".to_owned()]
    }

    #[cfg(windows)]
    fn runtime_exit_service_args() -> Vec<String> {
        vec!["/C".to_owned(), "timeout /T 5 /NOBREAK >NUL".to_owned()]
    }

    #[cfg(windows)]
    fn failing_action_config() -> &'static str {
        r#"
actions:
  prepare:
    executable: "cmd"
    args: ["/C", "exit 1"]
services:
  api:
    executable: "cmd"
    args: ["/C", "timeout /T 5 /NOBREAK >NUL"]
    hooks:
      before_start: ["prepare"]
"#
    }

    #[cfg(unix)]
    fn timeout_action_config() -> &'static str {
        r#"
actions:
  prepare:
    executable: "sh"
    args: ["-c", "sleep 2"]
    timeout_secs: 1
services:
  api:
    executable: "sh"
    args: ["-c", "sleep 5"]
    hooks:
      before_start: ["prepare"]
"#
    }

    #[cfg(windows)]
    fn timeout_action_config() -> &'static str {
        r#"
actions:
  prepare:
    executable: "cmd"
    args: ["/C", "timeout /T 2 /NOBREAK >NUL"]
    timeout_secs: 1
services:
  api:
    executable: "cmd"
    args: ["/C", "timeout /T 5 /NOBREAK >NUL"]
    hooks:
      before_start: ["prepare"]
"#
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
