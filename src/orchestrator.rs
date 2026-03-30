use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
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

use crate::cli::{KeyValueArg, ListArgs, RunArgs};
use crate::config::{
    ActionConfig, ActionRenderContext, HookName, LogConfig, LogOverflowStrategy,
    PreparedActionExecution, ProjectConfig, ResolvedActionConfig, ResolvedLogConfig,
    ResolvedServiceConfig, RestartPolicy, ServiceConfig, ServiceWatchConfig,
    is_known_placeholder_name,
};
use crate::error::{AppError, AppResult};
use crate::file_log::{FileLogSink, SharedFileLogSink};
use crate::process::{
    self, ActionRunStatus, CaptureOptions, OutputMode, SpawnedProcess, StopOutcome,
};
use crate::runtime_state::{self, RegistryEntry, RuntimeEvent, RuntimeLock, RuntimeState};
use crate::watch::{self, WatchHandle};

pub struct RunPlan {
    pub project_root: PathBuf,
    pub config_path: PathBuf,
    pub actions: BTreeMap<String, ResolvedActionConfig>,
    pub instance_log: Option<ResolvedLogConfig>,
    pub services: Vec<ResolvedServiceConfig>,
}

pub struct RunOptions {
    pub tui: bool,
    pub keep_tui: bool,
    pub manage_tui: bool,
    pub daemonized: bool,
}

#[derive(Clone)]
pub(crate) enum RuntimeOutputContext {
    Plain,
    Daemon,
    Tui(mpsc::Sender<process::LogEvent>),
}

#[derive(Debug, Clone)]
pub struct SingleRunRequest {
    target: RunTarget,
}

#[derive(Debug, Clone)]
enum RunTarget {
    Service {
        service_name: String,
        hook_selection: HookSelection,
    },
    Action {
        action_name: String,
        args: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum HookSelection {
    None,
    All,
    Selected(BTreeSet<HookName>),
}

impl HookSelection {
    fn allows(&self, hook: HookName) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Selected(selected) => selected.contains(&hook),
        }
    }
}

impl SingleRunRequest {
    pub fn from_args(args: RunArgs) -> AppResult<Self> {
        let target = match (args.service, args.action) {
            (Some(service_name), None) => RunTarget::Service {
                service_name,
                hook_selection: HookSelection::from_args(
                    args.with_all_hooks,
                    args.without_hooks,
                    args.hook,
                )?,
            },
            (None, Some(action_name)) => RunTarget::Action {
                action_name,
                args: collect_standalone_action_args(args.args)?,
            },
            _ => {
                return Err(AppError::config_invalid(
                    "run command requires exactly one of `--service` or `--action`",
                ));
            }
        };

        Ok(Self { target })
    }
}

impl HookSelection {
    fn from_args(with_all_hooks: bool, without_hooks: bool, hooks: Vec<String>) -> AppResult<Self> {
        if with_all_hooks {
            return Ok(Self::All);
        }
        if without_hooks || hooks.is_empty() {
            return Ok(Self::None);
        }

        let mut selected = BTreeSet::new();
        for hook in hooks {
            let parsed = HookName::parse(&hook).ok_or_else(|| {
                AppError::config_invalid(format!(
                    "unknown hook `{hook}`; expected one of: before_start, after_start_success, after_start_failure, before_stop, after_stop_success, after_stop_timeout, after_stop_failure, after_runtime_exit_unexpected"
                ))
            })?;
            selected.insert(parsed);
        }
        Ok(Self::Selected(selected))
    }
}

fn collect_standalone_action_args(args: Vec<KeyValueArg>) -> AppResult<BTreeMap<String, String>> {
    let mut collected = BTreeMap::new();
    for KeyValueArg { key, value } in args {
        if !is_known_placeholder_name(&key) {
            return Err(AppError::config_invalid(format!(
                "unknown standalone action arg `{key}`"
            )));
        }
        collected.insert(key, value);
    }
    Ok(collected)
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

pub fn run_list(_config_path: &Path, config: &ProjectConfig, args: ListArgs) -> AppResult<()> {
    let request = ListRequest::from_args(args);
    let output = render_list_output(config, &request);
    println!("{output}");
    Ok(())
}

pub fn run_single(
    config_path: &Path,
    config: &ProjectConfig,
    request: SingleRunRequest,
) -> AppResult<()> {
    match request.target {
        RunTarget::Service {
            service_name,
            hook_selection,
        } => run_single_service(config_path, config, &service_name, &hook_selection),
        RunTarget::Action { action_name, args } => {
            run_single_action(config_path, config, &action_name, &args)
        }
    }
}

fn resolve_run_context(config_path: &Path) -> AppResult<(PathBuf, PathBuf)> {
    let config_path = canonical_or_owned(config_path)?;
    let project_root = config_path
        .parent()
        .ok_or_else(|| AppError::config_invalid("configuration file must have a parent directory"))?
        .to_path_buf();
    Ok((config_path, project_root))
}

fn standalone_action_context(
    config: &ProjectConfig,
    config_path: &Path,
    project_root: &Path,
    action_name: &str,
    overrides: &BTreeMap<String, String>,
) -> AppResult<ActionRenderContext> {
    let effective_project_root = overrides
        .get("project_root")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.to_path_buf());
    let effective_config_path = overrides
        .get("config_path")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_path.to_path_buf());
    let inferred_service = overrides
        .get("service_name")
        .and_then(|service_name| config.resolve_service(service_name, project_root).ok());

    let service_name = overrides
        .get("service_name")
        .cloned()
        .unwrap_or_else(|| "manual".to_owned());
    let service_cwd = overrides
        .get("service_cwd")
        .map(PathBuf::from)
        .or_else(|| inferred_service.as_ref().map(|service| service.cwd.clone()))
        .unwrap_or_else(|| effective_project_root.clone());
    let service_executable = overrides
        .get("service_executable")
        .cloned()
        .or_else(|| {
            inferred_service
                .as_ref()
                .map(|service| service.executable.clone())
        })
        .unwrap_or_default();

    Ok(ActionRenderContext {
        project_root: effective_project_root,
        config_path: effective_config_path,
        service_name,
        action_name: overrides
            .get("action_name")
            .cloned()
            .unwrap_or_else(|| action_name.to_owned()),
        hook_name: overrides
            .get("hook_name")
            .cloned()
            .unwrap_or_else(|| "manual".to_owned()),
        service_cwd,
        service_executable,
        service_pid: Some(overrides.get("service_pid").cloned().unwrap_or_default()),
        stop_reason: Some(
            overrides
                .get("stop_reason")
                .cloned()
                .unwrap_or_else(|| "manual".to_owned()),
        ),
        exit_code: Some(overrides.get("exit_code").cloned().unwrap_or_default()),
        exit_status: Some(
            overrides
                .get("exit_status")
                .cloned()
                .unwrap_or_else(|| "manual".to_owned()),
        ),
    })
}

fn run_single_action(
    config_path: &Path,
    config: &ProjectConfig,
    action_name: &str,
    overrides: &BTreeMap<String, String>,
) -> AppResult<()> {
    let (config_path, project_root) = resolve_run_context(config_path)?;
    config.action_executable_exists(action_name, &project_root)?;
    let actions = config.resolve_actions(&project_root)?;
    let action = actions.get(action_name).ok_or_else(|| {
        AppError::config_invalid(format!("action `{action_name}` not found in configuration"))
    })?;
    let context =
        standalone_action_context(config, &config_path, &project_root, action_name, overrides)?;
    let (rendered_action, prepared) = prepare_action_execution(action, &context)?;
    announce_action_params(
        None,
        &context.service_name,
        &context.hook_name,
        &action.name,
        &prepared.resolved_params,
        HookOutputMode::Terminal,
    );

    match process::run_action(&rendered_action, &context.service_name, &context.hook_name)? {
        ActionRunStatus::Succeeded => {
            println!("action `{}` finished successfully", action.name);
            Ok(())
        }
        ActionRunStatus::Failed { status } => Err(AppError::startup_failed(format!(
            "action `{}` exited with status {status}",
            action.name
        ))),
        ActionRunStatus::TimedOut { timeout_secs } => Err(AppError::startup_failed(format!(
            "action `{}` timed out after {} seconds",
            action.name, timeout_secs
        ))),
    }
}

fn run_single_service(
    config_path: &Path,
    config: &ProjectConfig,
    service_name: &str,
    hook_selection: &HookSelection,
) -> AppResult<()> {
    let (config_path, project_root) = resolve_run_context(config_path)?;
    config.executable_exists(service_name, &project_root)?;
    let actions = config.resolve_actions(&project_root)?;
    let service = config.resolve_service(service_name, &project_root)?;
    let shutdown = install_shutdown_controller()?;

    if hook_selection.allows(HookName::BeforeStart) {
        execute_hook_actions(
            None,
            &config_path,
            &actions,
            &service,
            HookName::BeforeStart,
            HookRuntimeExtras {
                service_pid: None,
                stop_reason: None,
                exit_code: None,
                exit_status: None,
            },
            HookOutputMode::Terminal,
        )?;
    }

    let mut spawned = match process::spawn_service(
        &service,
        OutputMode::Capture(CaptureOptions {
            event_sender: None,
            log: service.log.clone(),
            echo_to_terminal: true,
        }),
    ) {
        Ok(spawned) => spawned,
        Err(error) => {
            if hook_selection.allows(HookName::AfterStartFailure) {
                let _ = execute_hook_actions(
                    None,
                    &config_path,
                    &actions,
                    &service,
                    HookName::AfterStartFailure,
                    HookRuntimeExtras {
                        service_pid: None,
                        stop_reason: None,
                        exit_code: None,
                        exit_status: Some(error.to_string()),
                    },
                    HookOutputMode::Terminal,
                );
            }
            return Err(error);
        }
    };

    println!(
        "started service `{}` with pid {}",
        service.name, spawned.state.pid
    );

    if hook_selection.allows(HookName::AfterStartSuccess) {
        if let Err(error) = execute_hook_actions(
            None,
            &config_path,
            &actions,
            &service,
            HookName::AfterStartSuccess,
            HookRuntimeExtras {
                service_pid: Some(spawned.state.pid),
                stop_reason: None,
                exit_code: Some(0),
                exit_status: Some("running".to_owned()),
            },
            HookOutputMode::Terminal,
        ) {
            eprintln!("{error}");
        }
    }

    loop {
        if shutdown.shutdown_requested() {
            eprintln!("interrupt received, stopping service. press Ctrl-C again to force.");
            stop_single_service(
                &mut spawned,
                &shutdown,
                &service,
                &actions,
                &config_path,
                hook_selection,
                "ctrl_c",
            )?;
            return Ok(());
        }

        if let Some(status) = process::service_exited(&mut spawned.child)? {
            if hook_selection.allows(HookName::AfterRuntimeExitUnexpected) {
                let _ = execute_hook_actions(
                    None,
                    &config_path,
                    &actions,
                    &service,
                    HookName::AfterRuntimeExitUnexpected,
                    HookRuntimeExtras {
                        service_pid: Some(spawned.state.pid),
                        stop_reason: None,
                        exit_code: status.code(),
                        exit_status: Some(status.to_string()),
                    },
                    HookOutputMode::Terminal,
                );
            }
            return Err(AppError::runtime_failed(format!(
                "service `{}` exited with status {status}",
                service.name
            )));
        }

        thread::sleep(Duration::from_millis(250));
    }
}

fn stop_single_service(
    process: &mut SpawnedProcess,
    shutdown: &ShutdownController,
    service: &ResolvedServiceConfig,
    actions: &BTreeMap<String, ResolvedActionConfig>,
    config_path: &Path,
    hook_selection: &HookSelection,
    stop_reason: &str,
) -> AppResult<()> {
    if process::service_exited(&mut process.child)?.is_some() {
        return Ok(());
    }

    if hook_selection.allows(HookName::BeforeStop) {
        if let Err(error) = execute_hook_actions(
            None,
            config_path,
            actions,
            service,
            HookName::BeforeStop,
            HookRuntimeExtras {
                service_pid: Some(process.state.pid),
                stop_reason: Some(stop_reason.to_owned()),
                exit_code: None,
                exit_status: None,
            },
            HookOutputMode::Terminal,
        ) {
            eprintln!("{error}");
        }
    }

    if shutdown.force_requested() {
        force_kill_process(process)?;
        let _ = process.child.wait();
        return run_single_service_stop_success(
            service,
            actions,
            config_path,
            hook_selection,
            process.state.pid,
            stop_reason,
            "forced",
            None,
        );
    }

    if let Err(error) = process::request_stop_service(&process.state) {
        if hook_selection.allows(HookName::AfterStopFailure) {
            if let Err(hook_error) = execute_hook_actions(
                None,
                config_path,
                actions,
                service,
                HookName::AfterStopFailure,
                HookRuntimeExtras {
                    service_pid: Some(process.state.pid),
                    stop_reason: Some(stop_reason.to_owned()),
                    exit_code: None,
                    exit_status: Some("request_stop_failed".to_owned()),
                },
                HookOutputMode::Terminal,
            ) {
                eprintln!("{hook_error}");
            }
        }
        return Err(error);
    }

    let deadline = Instant::now() + Duration::from_secs(process.state.stop_timeout_secs);
    loop {
        if process::service_exited(&mut process.child)?.is_some() {
            return run_single_service_stop_success(
                service,
                actions,
                config_path,
                hook_selection,
                process.state.pid,
                stop_reason,
                "graceful",
                Some(0),
            );
        }

        if shutdown.force_requested() {
            force_kill_process(process)?;
            let _ = process.child.wait();
            return run_single_service_stop_success(
                service,
                actions,
                config_path,
                hook_selection,
                process.state.pid,
                stop_reason,
                "forced",
                None,
            );
        }

        if Instant::now() >= deadline {
            if hook_selection.allows(HookName::AfterStopTimeout) {
                if let Err(error) = execute_hook_actions(
                    None,
                    config_path,
                    actions,
                    service,
                    HookName::AfterStopTimeout,
                    HookRuntimeExtras {
                        service_pid: Some(process.state.pid),
                        stop_reason: Some(stop_reason.to_owned()),
                        exit_code: None,
                        exit_status: Some("timeout".to_owned()),
                    },
                    HookOutputMode::Terminal,
                ) {
                    eprintln!("{error}");
                }
            }

            let force_result = force_kill_process(process);
            let _ = process.child.wait();
            return match force_result {
                Ok(()) => run_single_service_stop_success(
                    service,
                    actions,
                    config_path,
                    hook_selection,
                    process.state.pid,
                    stop_reason,
                    "timed_out_then_forced",
                    None,
                ),
                Err(error) => {
                    if hook_selection.allows(HookName::AfterStopFailure) {
                        if let Err(hook_error) = execute_hook_actions(
                            None,
                            config_path,
                            actions,
                            service,
                            HookName::AfterStopFailure,
                            HookRuntimeExtras {
                                service_pid: Some(process.state.pid),
                                stop_reason: Some(stop_reason.to_owned()),
                                exit_code: None,
                                exit_status: Some("force_stop_failed".to_owned()),
                            },
                            HookOutputMode::Terminal,
                        ) {
                            eprintln!("{hook_error}");
                        }
                    }
                    Err(error)
                }
            };
        }

        thread::sleep(Duration::from_millis(200));
    }
}

fn run_single_service_stop_success(
    service: &ResolvedServiceConfig,
    actions: &BTreeMap<String, ResolvedActionConfig>,
    config_path: &Path,
    hook_selection: &HookSelection,
    service_pid: u32,
    stop_reason: &str,
    exit_status: &str,
    exit_code: Option<i32>,
) -> AppResult<()> {
    if hook_selection.allows(HookName::AfterStopSuccess) {
        if let Err(error) = execute_hook_actions(
            None,
            config_path,
            actions,
            service,
            HookName::AfterStopSuccess,
            HookRuntimeExtras {
                service_pid: Some(service_pid),
                stop_reason: Some(stop_reason.to_owned()),
                exit_code,
                exit_status: Some(exit_status.to_owned()),
            },
            HookOutputMode::Terminal,
        ) {
            eprintln!("{error}");
        }
    }

    println!(
        "service `{}` stopped with outcome `{}`",
        service.name, exit_status
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ListScope {
    All,
    Services,
    Actions,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ListMode {
    Names,
    Detail,
    Dag,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct ListRequest {
    scope: ListScope,
    mode: ListMode,
}

impl ListRequest {
    fn from_args(args: ListArgs) -> Self {
        let scope = match (args.all, args.services, args.actions) {
            (_, true, true) | (true, _, _) | (false, false, false) => ListScope::All,
            (false, true, false) => ListScope::Services,
            (false, false, true) => ListScope::Actions,
        };

        let mode = if args.dag {
            ListMode::Dag
        } else if args.detail {
            ListMode::Detail
        } else {
            ListMode::Names
        };

        Self { scope, mode }
    }
}

fn render_list_output(config: &ProjectConfig, request: &ListRequest) -> String {
    match request.mode {
        ListMode::Names => render_list_names(config, request.scope),
        ListMode::Detail => render_list_detail(config, request.scope),
        ListMode::Dag => render_list_dag(config),
    }
}

fn render_list_names(config: &ProjectConfig, scope: ListScope) -> String {
    let mut sections = Vec::new();

    if matches!(scope, ListScope::All | ListScope::Services) {
        sections.push(render_name_section(
            "services",
            config
                .services
                .iter()
                .map(|(name, service)| format_name_item(name, service.disabled.unwrap_or(false)))
                .collect(),
        ));
    }

    if matches!(scope, ListScope::All | ListScope::Actions) {
        sections.push(render_name_section(
            "actions",
            config
                .actions
                .iter()
                .map(|(name, action)| format_name_item(name, action.disabled.unwrap_or(false)))
                .collect(),
        ));
    }

    sections.join("\n\n")
}

fn render_list_detail(config: &ProjectConfig, scope: ListScope) -> String {
    let mut sections = Vec::new();

    if matches!(scope, ListScope::All | ListScope::Services) {
        let mut body = String::from("services:\n");
        if config.services.is_empty() {
            body.push_str("- (none)\n");
        } else {
            for (index, (name, service)) in config.services.iter().enumerate() {
                if index > 0 {
                    body.push('\n');
                }
                append_service_detail(&mut body, name, service);
            }
        }
        trim_trailing_newline(&mut body);
        sections.push(body);
    }

    if matches!(scope, ListScope::All | ListScope::Actions) {
        let mut body = String::from("actions:\n");
        if config.actions.is_empty() {
            body.push_str("- (none)\n");
        } else {
            for (index, (name, action)) in config.actions.iter().enumerate() {
                if index > 0 {
                    body.push('\n');
                }
                append_action_detail(&mut body, name, action);
            }
        }
        trim_trailing_newline(&mut body);
        sections.push(body);
    }

    sections.join("\n\n")
}

fn render_list_dag(config: &ProjectConfig) -> String {
    let mut sections = Vec::new();
    let mut dependency_lines = Vec::new();
    let mut hook_lines = Vec::new();
    let mut referenced_actions = BTreeSet::new();

    for (service_name, service) in &config.services {
        let service_label = format_service_label(service_name, service.disabled.unwrap_or(false));
        for dependency in &service.depends_on {
            let dependency_label = config
                .services
                .get(dependency)
                .map(|service| format_service_label(dependency, service.disabled.unwrap_or(false)))
                .unwrap_or_else(|| format_service_label(dependency, false));
            dependency_lines.push(format!(
                "- service: {service_label} --depends_on--> service: {dependency_label}"
            ));
        }

        for hook in HookName::all() {
            for action_name in service.hooks.actions_for(hook) {
                referenced_actions.insert(action_name.clone());
                let action_label = config
                    .actions
                    .get(action_name)
                    .map(|action| {
                        format_action_label(action_name, action.disabled.unwrap_or(false))
                    })
                    .unwrap_or_else(|| format_action_label(action_name, false));
                hook_lines.push(format!(
                    "- service: {service_label} --hooks.{}--> action: {action_label}",
                    hook.as_str()
                ));
            }
        }
    }

    sections.push(render_plain_section(
        "service dependencies",
        dependency_lines,
    ));
    sections.push(render_plain_section("hook references", hook_lines));

    let standalone_actions = config
        .actions
        .iter()
        .filter(|(name, _)| !referenced_actions.contains(*name))
        .map(|(name, action)| format_name_item(name, action.disabled.unwrap_or(false)))
        .collect::<Vec<_>>();
    if !standalone_actions.is_empty() {
        sections.push(render_name_section(
            "standalone actions",
            standalone_actions,
        ));
    }

    sections.join("\n\n")
}

fn append_service_detail(output: &mut String, name: &str, service: &ServiceConfig) {
    let _ = writeln!(
        output,
        "- {}{}",
        name,
        format_disabled_suffix(service.disabled.unwrap_or(false))
    );
    let _ = writeln!(output, "  executable: {}", service.executable);
    let _ = writeln!(output, "  args: {:?}", service.args);
    let _ = writeln!(
        output,
        "  cwd: {}",
        format_optional_path(service.cwd.as_ref())
    );
    let _ = writeln!(output, "  env: {}", format_string_map(&service.env));
    let _ = writeln!(output, "  depends_on: {:?}", service.depends_on);
    let _ = writeln!(
        output,
        "  restart: {}",
        format_restart_policy(service.restart.as_ref())
    );
    let _ = writeln!(
        output,
        "  stop_signal: {}",
        format_optional_string(service.stop_signal.as_ref())
    );
    let _ = writeln!(
        output,
        "  stop_timeout_secs: {}",
        format_optional_u64(service.stop_timeout_secs)
    );
    let _ = writeln!(
        output,
        "  autostart: {}",
        format_optional_bool(service.autostart)
    );
    let _ = writeln!(output, "  disabled: {}", service.disabled.unwrap_or(false));
    let _ = writeln!(output, "  log: {}", format_log_config(service.log.as_ref()));
    let _ = writeln!(
        output,
        "  watch: {}",
        format_watch_config(service.watch.as_ref())
    );
    append_hooks_detail(output, &service.hooks);
}

fn append_action_detail(output: &mut String, name: &str, action: &ActionConfig) {
    let _ = writeln!(
        output,
        "- {}{}",
        name,
        format_disabled_suffix(action.disabled.unwrap_or(false))
    );
    let _ = writeln!(output, "  executable: {}", action.executable);
    let _ = writeln!(output, "  args: {:?}", action.args);
    let _ = writeln!(
        output,
        "  cwd: {}",
        format_optional_path(action.cwd.as_ref())
    );
    let _ = writeln!(output, "  env: {}", format_string_map(&action.env));
    let _ = writeln!(
        output,
        "  timeout_secs: {}",
        format_optional_u64(action.timeout_secs)
    );
    let _ = writeln!(output, "  disabled: {}", action.disabled.unwrap_or(false));
}

fn append_hooks_detail(output: &mut String, hooks: &crate::config::ServiceHooksConfig) {
    if hooks.before_start.is_empty()
        && hooks.after_start_success.is_empty()
        && hooks.after_start_failure.is_empty()
        && hooks.before_stop.is_empty()
        && hooks.after_stop_success.is_empty()
        && hooks.after_stop_timeout.is_empty()
        && hooks.after_stop_failure.is_empty()
        && hooks.after_runtime_exit_unexpected.is_empty()
    {
        let _ = writeln!(output, "  hooks: none");
        return;
    }

    let _ = writeln!(output, "  hooks:");
    for hook in HookName::all() {
        let actions = hooks.actions_for(hook);
        if actions.is_empty() {
            continue;
        }
        let _ = writeln!(output, "    {}: {:?}", hook.as_str(), actions);
    }
}

fn render_name_section(title: &str, items: Vec<String>) -> String {
    render_plain_section(
        title,
        if items.is_empty() {
            vec!["- (none)".to_owned()]
        } else {
            items.into_iter().map(|item| format!("- {item}")).collect()
        },
    )
}

fn render_plain_section(title: &str, items: Vec<String>) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "{title}:");
    if items.is_empty() {
        output.push_str("- (none)\n");
    } else {
        for item in items {
            let _ = writeln!(output, "{item}");
        }
    }
    trim_trailing_newline(&mut output);
    output
}

fn format_name_item(name: &str, disabled: bool) -> String {
    format!("{name}{}", format_disabled_suffix(disabled))
}

fn format_service_label(name: &str, disabled: bool) -> String {
    format_name_item(name, disabled)
}

fn format_action_label(name: &str, disabled: bool) -> String {
    format_name_item(name, disabled)
}

fn format_disabled_suffix(disabled: bool) -> &'static str {
    if disabled { " [disabled]" } else { "" }
}

fn format_optional_path(path: Option<&PathBuf>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "none".to_owned())
}

fn format_string_map(values: &BTreeMap<String, String>) -> String {
    if values.is_empty() {
        "{}".to_owned()
    } else {
        format!("{values:?}")
    }
}

fn format_optional_string(value: Option<&String>) -> String {
    value.cloned().unwrap_or_else(|| "none".to_owned())
}

fn format_optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_owned())
}

fn format_optional_bool(value: Option<bool>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_owned())
}

fn format_restart_policy(policy: Option<&RestartPolicy>) -> String {
    match policy {
        Some(RestartPolicy::No) => "no".to_owned(),
        Some(RestartPolicy::OnFailure) => "on-failure".to_owned(),
        Some(RestartPolicy::Always) => "always".to_owned(),
        None => "none".to_owned(),
    }
}

fn format_log_config(log: Option<&LogConfig>) -> String {
    let Some(log) = log else {
        return "none".to_owned();
    };

    format!(
        "file={} append={} max_file_bytes={} overflow_strategy={} rotate_file_count={}",
        log.file.display(),
        log.append,
        format_optional_u64(log.max_file_bytes),
        format_overflow_strategy(log.overflow_strategy.as_ref()),
        log.rotate_file_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "none".to_owned())
    )
}

fn format_watch_config(watch: Option<&ServiceWatchConfig>) -> String {
    let Some(watch) = watch else {
        return "none".to_owned();
    };

    format!(
        "paths={:?} debounce_ms={}",
        watch.paths,
        format_optional_u64(watch.debounce_ms)
    )
}

fn format_overflow_strategy(strategy: Option<&LogOverflowStrategy>) -> String {
    match strategy {
        Some(LogOverflowStrategy::Rotate) => "rotate".to_owned(),
        Some(LogOverflowStrategy::Archive) => "archive".to_owned(),
        None => "none".to_owned(),
    }
}

fn trim_trailing_newline(output: &mut String) {
    if output.ends_with('\n') {
        output.pop();
    }
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
        return run_up_tui(plan, options.keep_tui, options.manage_tui);
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
    let output_context = RuntimeOutputContext::Plain;
    let mut running = match start_services(&plan, &mut runtime_state, &output_context) {
        Ok(running) => running,
        Err(error) => {
            let cleanup_result = runtime_state::cleanup_runtime_files(&plan.project_root)
                .and_then(|_| lock.release());
            cleanup_result?;
            return Err(error);
        }
    };
    let mut watch_runtime = WatchRuntime::start(&plan)?;
    runtime_state::register_instance(&runtime_state)?;

    render_plain_status(&running, Duration::from_secs(0))?;

    let runtime_result = monitor_plain_processes(
        &plan,
        &mut running,
        &mut runtime_state,
        watch_runtime.as_mut(),
        &shutdown,
        &output_context,
    );
    drop(watch_runtime);
    let stop_reason = determine_stop_reason(&runtime_result, &shutdown, "ctrl_c");
    emit_runtime_event(
        &plan.project_root,
        "instance_stopping",
        None,
        None,
        None,
        format_detail_fields(&[("reason", stop_reason.to_owned())]),
    );
    let shutdown_result =
        shutdown_running_services(&mut running, &shutdown, &plan, stop_reason, &output_context);

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

fn run_up_tui(plan: RunPlan, keep_tui: bool, manage_tui: bool) -> AppResult<()> {
    let _instance_log_guard = install_instance_logger(plan.instance_log.clone())?;
    let shutdown = install_shutdown_controller()?;
    let (log_tx, log_rx) = mpsc::channel();
    let output_context = RuntimeOutputContext::Tui(log_tx.clone());

    let lock = RuntimeLock::acquire(&plan.project_root)?;
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
            ("keep", keep_tui.to_string()),
            ("manage", manage_tui.to_string()),
            ("config", plan.config_path.display().to_string()),
            ("service_count", plan.services.len().to_string()),
        ]),
    );

    let mut running = match start_services(&plan, &mut runtime_state, &output_context) {
        Ok(running) => running,
        Err(error) => {
            let cleanup_result = runtime_state::cleanup_runtime_files(&plan.project_root)
                .and_then(|_| lock.release());
            cleanup_result?;
            return Err(error);
        }
    };
    let mut watch_runtime = WatchRuntime::start(&plan)?;
    runtime_state::register_instance(&runtime_state)?;
    let mut session = crate::tui::DashboardSession::enter(&plan.services, &running)?;

    let runtime_result = session.run_running_phase(
        &plan,
        &mut running,
        &mut runtime_state,
        watch_runtime.as_mut(),
        &log_rx,
        shutdown.clone(),
        &output_context,
    );
    drop(watch_runtime);
    let stop_reason = determine_stop_reason(&runtime_result, &shutdown, "ctrl_c");
    emit_runtime_event(
        &plan.project_root,
        "instance_stopping",
        None,
        None,
        None,
        format_detail_fields(&[("reason", stop_reason.to_owned())]),
    );
    let shutdown_result =
        shutdown_running_services(&mut running, &shutdown, &plan, stop_reason, &output_context);
    running.clear();

    let post_run_phase = if manage_tui {
        crate::tui::DashboardPhase::PostRunManage
    } else {
        crate::tui::DashboardPhase::PostRunReadonly
    };
    if keep_tui {
        session.freeze_post_run(
            post_run_phase,
            &plan.project_root,
            &plan.services,
            &running,
            &log_rx,
            keep_mode_notice(manage_tui),
        )?;
    }

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

    if keep_tui {
        session.redraw()?;
        loop {
            match session.run_post_run_phase(&shutdown)? {
                crate::tui::PostRunAction::Exit => break,
                crate::tui::PostRunAction::RestartSelectedService(service_name) => {
                    if let Err(error) = run_tui_managed_cycle(
                        &plan,
                        &service_name,
                        &mut session,
                        &shutdown,
                        &log_tx,
                        &log_rx,
                    ) {
                        session.set_notice(format!(
                            "failed to start or restart {service_name}: {error}"
                        ));
                    }
                }
            }
        }
    }

    session.exit()?;
    runtime_result?;
    shutdown_result?;
    cleanup_result?;

    Ok(())
}

fn keep_mode_notice(manage_tui: bool) -> &'static str {
    if manage_tui {
        "post-run manage mode; press R to start or restart the selected service, q or Esc to exit"
    } else {
        "post-run view; press q or Esc to exit"
    }
}

fn run_tui_managed_cycle(
    plan: &RunPlan,
    service_name: &str,
    session: &mut crate::tui::DashboardSession,
    shutdown: &ShutdownController,
    log_tx: &mpsc::Sender<process::LogEvent>,
    log_rx: &mpsc::Receiver<process::LogEvent>,
) -> AppResult<()> {
    shutdown.reset();

    let output_context = RuntimeOutputContext::Tui(log_tx.clone());
    let lock = RuntimeLock::acquire(&plan.project_root)?;
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
            ("mode", "tui_manage".to_owned()),
            ("config", plan.config_path.display().to_string()),
            ("service_count", "1".to_owned()),
            ("service", service_name.to_owned()),
        ]),
    );

    let mut running = Vec::new();
    let restart_outcome = restart_service(
        plan,
        service_name,
        ServiceRestartTrigger::Tui,
        &mut running,
        &mut runtime_state,
        shutdown,
        &output_context,
    )?;
    match restart_outcome {
        ServiceRestartOutcome::Restarted => {}
        ServiceRestartOutcome::Skipped { detail } => {
            let cleanup_result = runtime_state::cleanup_runtime_files(&plan.project_root)
                .and_then(|_| lock.release());
            cleanup_result?;
            return Err(AppError::runtime_failed(detail));
        }
    }

    if let Err(error) = runtime_state::register_instance(&runtime_state) {
        let shutdown_result = shutdown_running_services(
            &mut running,
            shutdown,
            plan,
            "register_failure",
            &output_context,
        );
        let cleanup_result =
            runtime_state::cleanup_runtime_files(&plan.project_root).and_then(|_| lock.release());
        shutdown_result?;
        cleanup_result?;
        return Err(error);
    }

    session.set_notice(format!("service {service_name} restarted"));
    let runtime_result = session.run_running_phase(
        plan,
        &mut running,
        &mut runtime_state,
        None,
        log_rx,
        shutdown.clone(),
        &output_context,
    );

    let stop_reason = determine_stop_reason(&runtime_result, shutdown, "ctrl_c");
    emit_runtime_event(
        &plan.project_root,
        "instance_stopping",
        None,
        None,
        None,
        format_detail_fields(&[("reason", stop_reason.to_owned())]),
    );
    let shutdown_result =
        shutdown_running_services(&mut running, shutdown, plan, stop_reason, &output_context);
    running.clear();
    session.freeze_post_run(
        crate::tui::DashboardPhase::PostRunManage,
        &plan.project_root,
        &plan.services,
        &running,
        log_rx,
        managed_cycle_notice(service_name, &runtime_result, &shutdown_result),
    )?;
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
    cleanup_result?;
    session.redraw()?;

    Ok(())
}

fn managed_cycle_notice(
    service_name: &str,
    runtime_result: &AppResult<()>,
    shutdown_result: &AppResult<()>,
) -> String {
    if let Err(error) = runtime_result {
        format!(
            "post-run manage mode; last run for {service_name} ended with error: {error}. press R to try again, q or Esc to exit"
        )
    } else if let Err(error) = shutdown_result {
        format!(
            "post-run manage mode; shutdown for {service_name} reported an error: {error}. press R to try again, q or Esc to exit"
        )
    } else {
        keep_mode_notice(true).to_owned()
    }
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
    let output_context = RuntimeOutputContext::Daemon;
    let mut running = match start_services(&plan, &mut runtime_state, &output_context) {
        Ok(running) => running,
        Err(error) => {
            let cleanup_result = runtime_state::cleanup_runtime_files(&plan.project_root)
                .and_then(|_| lock.release());
            cleanup_result?;
            return Err(error);
        }
    };
    let mut watch_runtime = WatchRuntime::start(&plan)?;
    runtime_state::register_instance(&runtime_state)?;

    let runtime_result = monitor_daemon_processes(
        &plan,
        &mut running,
        &mut runtime_state,
        watch_runtime.as_mut(),
        &shutdown,
        &output_context,
    );
    drop(watch_runtime);
    let stop_reason = determine_stop_reason(&runtime_result, &shutdown, "ctrl_c");
    emit_runtime_event(
        &plan.project_root,
        "instance_stopping",
        None,
        None,
        None,
        format_detail_fields(&[("reason", stop_reason.to_owned())]),
    );
    let shutdown_result =
        shutdown_running_services(&mut running, &shutdown, &plan, stop_reason, &output_context);
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

fn output_mode_for_service(
    service: &ResolvedServiceConfig,
    output_context: &RuntimeOutputContext,
) -> OutputMode {
    match output_context {
        RuntimeOutputContext::Plain | RuntimeOutputContext::Daemon => {
            if service.log.is_some() {
                OutputMode::Capture(CaptureOptions {
                    event_sender: None,
                    log: service.log.clone(),
                    echo_to_terminal: false,
                })
            } else {
                OutputMode::Null
            }
        }
        RuntimeOutputContext::Tui(log_tx) => OutputMode::Capture(CaptureOptions {
            event_sender: Some(log_tx.clone()),
            log: service.log.clone(),
            echo_to_terminal: false,
        }),
    }
}

#[derive(Debug)]
struct PendingWatchRestart {
    changed_path: PathBuf,
    ready_at: Instant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ServiceRestartTrigger {
    Watch { changed_path: PathBuf },
    Tui,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ServiceRestartOutcome {
    Restarted,
    Skipped { detail: String },
}

pub struct WatchRuntime {
    raw_rx: mpsc::Receiver<watch::WatchEvent>,
    handles: Vec<WatchHandle>,
    pending: BTreeMap<String, PendingWatchRestart>,
}

impl WatchRuntime {
    fn start(plan: &RunPlan) -> AppResult<Option<Self>> {
        let watched_services = plan
            .services
            .iter()
            .filter(|service| service.watch.is_some())
            .collect::<Vec<_>>();
        if watched_services.is_empty() {
            return Ok(None);
        }

        let (tx, rx) = mpsc::channel();
        let ignore_paths = watch_ignore_paths(plan);
        let mut handles = Vec::with_capacity(watched_services.len());

        for service in watched_services {
            let watch = service
                .watch
                .as_ref()
                .expect("filtered service watch config must exist");
            handles.push(watch::spawn_service_watcher(
                watch::WatchRequest {
                    service_name: service.name.clone(),
                    paths: watch.paths.clone(),
                    ignore_paths: ignore_paths.clone(),
                    poll_interval: Duration::from_millis(200),
                },
                tx.clone(),
            ));
        }

        drop(tx);

        Ok(Some(Self {
            raw_rx: rx,
            handles,
            pending: BTreeMap::new(),
        }))
    }

    pub fn tick(
        &mut self,
        plan: &RunPlan,
        running: &mut Vec<SpawnedProcess>,
        runtime_state: &mut RuntimeState,
        shutdown: &ShutdownController,
        output_context: &RuntimeOutputContext,
    ) -> AppResult<()> {
        while let Ok(event) = self.raw_rx.try_recv() {
            let Some(service) = plan
                .services
                .iter()
                .find(|service| service.name == event.service_name)
            else {
                continue;
            };
            let Some(watch) = service.watch.as_ref() else {
                continue;
            };

            emit_runtime_event(
                &plan.project_root,
                "watch_triggered",
                Some(&service.name),
                None,
                None,
                format_detail_fields(&[
                    ("path", event.changed_path.display().to_string()),
                    ("debounce_ms", watch.debounce_ms.to_string()),
                ]),
            );
            self.pending.insert(
                service.name.clone(),
                PendingWatchRestart {
                    changed_path: event.changed_path,
                    ready_at: Instant::now() + Duration::from_millis(watch.debounce_ms),
                },
            );
        }

        if shutdown.shutdown_requested() {
            while let Some((service_name, pending)) = self.pending.pop_first() {
                emit_runtime_event(
                    &plan.project_root,
                    "service_restart_skipped",
                    Some(&service_name),
                    None,
                    None,
                    restart_event_detail(
                        &ServiceRestartTrigger::Watch {
                            changed_path: pending.changed_path,
                        },
                        &[
                            ("reason", "shutdown".to_owned()),
                            ("state", "pending".to_owned()),
                        ],
                    ),
                );
            }
            return Ok(());
        }

        let now = Instant::now();
        let due_service_name = self.pending.iter().find_map(|(service_name, pending)| {
            (pending.ready_at <= now).then(|| service_name.clone())
        });
        let Some(service_name) = due_service_name else {
            return Ok(());
        };
        let Some(pending) = self.pending.remove(&service_name) else {
            return Ok(());
        };

        emit_runtime_event(
            &plan.project_root,
            "watch_debounced",
            Some(&service_name),
            None,
            None,
            format_detail_fields(&[
                ("path", pending.changed_path.display().to_string()),
                ("state", "ready".to_owned()),
            ]),
        );

        restart_service(
            plan,
            &service_name,
            ServiceRestartTrigger::Watch {
                changed_path: pending.changed_path,
            },
            running,
            runtime_state,
            shutdown,
            output_context,
        )?;

        Ok(())
    }

    pub fn clear_pending(&mut self, service_name: &str) {
        self.pending.remove(service_name);
    }
}

impl Drop for WatchRuntime {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.stop();
        }
    }
}

fn watch_ignore_paths(plan: &RunPlan) -> Vec<PathBuf> {
    let mut ignored = BTreeSet::new();
    ignored.insert(plan.project_root.join(runtime_state::RUNTIME_DIR));
    collect_ignore_path(
        &mut ignored,
        &plan.project_root,
        plan.instance_log.as_ref().map(|log| &log.file),
    );
    for service in &plan.services {
        collect_ignore_path(
            &mut ignored,
            &plan.project_root,
            service.log.as_ref().map(|log| &log.file),
        );
    }
    ignored.into_iter().collect()
}

fn collect_ignore_path(
    ignored: &mut BTreeSet<PathBuf>,
    project_root: &Path,
    path: Option<&PathBuf>,
) {
    let Some(path) = path else {
        return;
    };

    ignored.insert(path.clone());
    let mut current = path.parent();
    while let Some(parent) = current {
        if parent == project_root {
            break;
        }
        ignored.insert(parent.to_path_buf());
        current = parent.parent();
    }
}

pub(crate) fn restart_service(
    plan: &RunPlan,
    service_name: &str,
    trigger: ServiceRestartTrigger,
    running: &mut Vec<SpawnedProcess>,
    runtime_state: &mut RuntimeState,
    shutdown: &ShutdownController,
    output_context: &RuntimeOutputContext,
) -> AppResult<ServiceRestartOutcome> {
    emit_runtime_event(
        &plan.project_root,
        "service_restart_requested",
        Some(service_name),
        None,
        None,
        restart_event_detail(&trigger, &[]),
    );
    emit_restart_notice(
        output_context,
        &trigger,
        &format_restart_notice(&trigger, service_name, "requested", None),
    );

    if shutdown.shutdown_requested() {
        let detail = "shutdown in progress".to_owned();
        emit_runtime_event(
            &plan.project_root,
            "service_restart_skipped",
            Some(service_name),
            None,
            None,
            restart_event_detail(&trigger, &[("reason", "shutdown".to_owned())]),
        );
        emit_restart_notice(
            output_context,
            &trigger,
            &format_restart_notice(&trigger, service_name, "skipped", Some(detail.as_str())),
        );
        return Ok(ServiceRestartOutcome::Skipped { detail });
    }

    let Some(service) = plan
        .services
        .iter()
        .find(|service| service.name == service_name)
    else {
        let detail = "service not found".to_owned();
        emit_runtime_event(
            &plan.project_root,
            "service_restart_skipped",
            Some(service_name),
            None,
            None,
            restart_event_detail(&trigger, &[("reason", "service_not_found".to_owned())]),
        );
        emit_restart_notice(
            output_context,
            &trigger,
            &format_restart_notice(&trigger, service_name, "skipped", Some(detail.as_str())),
        );
        return Ok(ServiceRestartOutcome::Skipped { detail });
    };

    if let Some(index) = running
        .iter()
        .position(|process| process.state.service_name == service_name)
    {
        let stop_result = {
            let mut force_notice_shown = false;
            stop_spawned_process(
                &mut running[index],
                shutdown,
                &mut force_notice_shown,
                plan,
                trigger.stop_reason(),
                output_context,
            )
        };
        match stop_result {
            Ok(()) => {
                running.remove(index);
                runtime_state
                    .services
                    .retain(|entry| entry.service_name != service_name);
                runtime_state::write_state(&plan.project_root, runtime_state)?;
            }
            Err(error) => {
                let detail = error.to_string();
                emit_runtime_event(
                    &plan.project_root,
                    "service_restart_skipped",
                    Some(service_name),
                    None,
                    None,
                    restart_event_detail(
                        &trigger,
                        &[
                            ("reason", "stop_failed".to_owned()),
                            ("detail", detail.clone()),
                        ],
                    ),
                );
                emit_restart_notice(
                    output_context,
                    &trigger,
                    &format_restart_notice(
                        &trigger,
                        service_name,
                        "skipped",
                        Some(detail.as_str()),
                    ),
                );
                return Ok(ServiceRestartOutcome::Skipped { detail });
            }
        }
    }

    match start_service(plan, runtime_state, service, output_context) {
        Ok(spawned) => {
            running.push(spawned);
            emit_runtime_event(
                &plan.project_root,
                "service_restart_succeeded",
                Some(service_name),
                None,
                None,
                restart_event_detail(&trigger, &[]),
            );
            emit_restart_notice(
                output_context,
                &trigger,
                &format_restart_notice(&trigger, service_name, "succeeded", None),
            );
            Ok(ServiceRestartOutcome::Restarted)
        }
        Err(error) => {
            let detail = error.to_string();
            runtime_state
                .services
                .retain(|entry| entry.service_name != service_name);
            runtime_state::write_state(&plan.project_root, runtime_state)?;
            emit_runtime_event(
                &plan.project_root,
                "service_restart_skipped",
                Some(service_name),
                None,
                None,
                restart_event_detail(
                    &trigger,
                    &[
                        ("reason", "start_failed".to_owned()),
                        ("detail", detail.clone()),
                    ],
                ),
            );
            emit_restart_notice(
                output_context,
                &trigger,
                &format_restart_notice(&trigger, service_name, "skipped", Some(detail.as_str())),
            );
            Ok(ServiceRestartOutcome::Skipped { detail })
        }
    }
}

fn restart_event_detail(trigger: &ServiceRestartTrigger, extra: &[(&str, String)]) -> String {
    let mut fields = vec![("trigger", trigger.name().to_owned())];
    match trigger {
        ServiceRestartTrigger::Watch { changed_path } => {
            fields.push(("path", changed_path.display().to_string()));
        }
        ServiceRestartTrigger::Tui => {
            fields.push(("key", "R".to_owned()));
        }
    }
    for (key, value) in extra {
        fields.push((*key, value.clone()));
    }
    format_detail_fields(&fields)
}

fn emit_restart_notice(
    output_context: &RuntimeOutputContext,
    trigger: &ServiceRestartTrigger,
    message: &str,
) {
    if !matches!(trigger, ServiceRestartTrigger::Watch { .. })
        || !matches!(output_context, RuntimeOutputContext::Plain)
    {
        return;
    }

    print!("\r\x1b[2K");
    println!("{message}");
}

fn format_restart_notice(
    trigger: &ServiceRestartTrigger,
    service_name: &str,
    phase: &str,
    detail: Option<&str>,
) -> String {
    match trigger {
        ServiceRestartTrigger::Watch { changed_path } => match phase {
            "requested" => format!(
                "watch: restarting {} because {} changed",
                service_name,
                changed_path.display()
            ),
            "succeeded" => format!("watch: restarted {}", service_name),
            "skipped" => match detail {
                Some(detail) => format!("watch: skipped restart for {} ({detail})", service_name),
                None => format!("watch: skipped restart for {}", service_name),
            },
            _ => format!("watch: restart {phase} for {}", service_name),
        },
        ServiceRestartTrigger::Tui => match phase {
            "requested" => format!("restarting {}...", service_name),
            "succeeded" => format!("service {} restarted", service_name),
            "skipped" => match detail {
                Some(detail) => format!("failed to restart {}: {}", service_name, detail),
                None => format!("failed to restart {}", service_name),
            },
            _ => format!("restart {phase} for {}", service_name),
        },
    }
}

impl ServiceRestartTrigger {
    fn name(&self) -> &'static str {
        match self {
            Self::Watch { .. } => "watch",
            Self::Tui => "tui",
        }
    }

    fn stop_reason(&self) -> &'static str {
        match self {
            Self::Watch { .. } => "watch_restart",
            Self::Tui => "tui_restart",
        }
    }
}

fn monitor_plain_processes(
    plan: &RunPlan,
    running: &mut Vec<SpawnedProcess>,
    runtime_state: &mut RuntimeState,
    watch_runtime: Option<&mut WatchRuntime>,
    shutdown: &ShutdownController,
    output_context: &RuntimeOutputContext,
) -> AppResult<()> {
    let started_at = std::time::Instant::now();
    let mut last_rendered_secs = 0;
    let mut watch_runtime = watch_runtime;

    loop {
        if shutdown.shutdown_requested() {
            println!();
            eprintln!("interrupt received, stopping services. press Ctrl-C again to force.");
            return Ok(());
        }

        if let Some(watch_runtime) = watch_runtime.as_mut() {
            watch_runtime.tick(plan, running, runtime_state, shutdown, output_context)?;
        }

        for process in running.iter_mut() {
            if let Some(status) = process::service_exited(&mut process.child)? {
                if !runtime_state::state_path(&plan.project_root).exists() {
                    return Ok(());
                }
                handle_runtime_exit(plan, process, status, output_context)?;
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

fn start_service(
    plan: &RunPlan,
    runtime_state: &mut RuntimeState,
    service: &ResolvedServiceConfig,
    output_context: &RuntimeOutputContext,
) -> AppResult<SpawnedProcess> {
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
        HookOutputMode::from_runtime_output(output_context),
    ) {
        emit_runtime_event(
            &plan.project_root,
            "service_start_aborted",
            Some(&service.name),
            Some(HookName::BeforeStart.as_str()),
            None,
            error.to_string(),
        );
        return Err(error);
    }

    let spawned =
        match process::spawn_service(service, output_mode_for_service(service, output_context)) {
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
                    HookOutputMode::from_runtime_output(output_context),
                ) {
                    report_nonfatal_hook_error(
                        HookOutputMode::from_runtime_output(output_context),
                        &hook_error,
                    );
                }
                return Err(error);
            }
        };
    runtime_state
        .services
        .retain(|entry| entry.service_name != service.name);
    runtime_state.services.push(spawned.state.clone());
    runtime_state::write_state(&plan.project_root, runtime_state)?;
    emit_runtime_event(
        &plan.project_root,
        "service_running",
        Some(&service.name),
        None,
        None,
        format!(
            "service `{}` entered running with pid {}",
            service.name, spawned.state.pid
        ),
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
        HookOutputMode::from_runtime_output(output_context),
    ) {
        report_nonfatal_hook_error(HookOutputMode::from_runtime_output(output_context), &error);
    }
    Ok(spawned)
}

fn start_services(
    plan: &RunPlan,
    runtime_state: &mut RuntimeState,
    output_context: &RuntimeOutputContext,
) -> AppResult<Vec<SpawnedProcess>> {
    let mut running = Vec::new();
    for service in &plan.services {
        match start_service(plan, runtime_state, service, output_context) {
            Ok(spawned) => running.push(spawned),
            Err(error) => {
                shutdown_running_services(
                    &mut running,
                    &ShutdownController::force_requested_now(),
                    plan,
                    "startup_failure",
                    output_context,
                )?;
                return Err(error);
            }
        }
    }
    Ok(running)
}

fn handle_runtime_exit(
    plan: &RunPlan,
    process: &mut SpawnedProcess,
    status: std::process::ExitStatus,
    output_context: &RuntimeOutputContext,
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
        HookOutputMode::from_runtime_output(output_context),
    ) {
        report_nonfatal_hook_error(HookOutputMode::from_runtime_output(output_context), &error);
    }

    Ok(())
}

fn monitor_daemon_processes(
    plan: &RunPlan,
    running: &mut Vec<SpawnedProcess>,
    runtime_state: &mut RuntimeState,
    watch_runtime: Option<&mut WatchRuntime>,
    shutdown: &ShutdownController,
    output_context: &RuntimeOutputContext,
) -> AppResult<()> {
    let mut watch_runtime = watch_runtime;

    loop {
        if shutdown.shutdown_requested() {
            return Ok(());
        }

        if let Some(watch_runtime) = watch_runtime.as_mut() {
            watch_runtime.tick(plan, running, runtime_state, shutdown, output_context)?;
        }

        for process in running.iter_mut() {
            if let Some(status) = process::service_exited(&mut process.child)? {
                if !runtime_state::state_path(&plan.project_root).exists() {
                    return Ok(());
                }
                handle_runtime_exit(plan, process, status, output_context)?;
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
    output_context: &RuntimeOutputContext,
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
            output_context,
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
    output_context: &RuntimeOutputContext,
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
        HookOutputMode::from_runtime_output(output_context),
    ) {
        report_nonfatal_hook_error(HookOutputMode::from_runtime_output(output_context), &error);
    }

    if shutdown.force_requested() {
        if !*force_notice_shown {
            if HookOutputMode::from_runtime_output(output_context) == HookOutputMode::Terminal {
                eprintln!("second interrupt received, force-stopping services.");
            }
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
            HookOutputMode::from_runtime_output(output_context),
        ) {
            report_nonfatal_hook_error(HookOutputMode::from_runtime_output(output_context), &error);
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
            HookOutputMode::from_runtime_output(output_context),
        ) {
            report_nonfatal_hook_error(
                HookOutputMode::from_runtime_output(output_context),
                &hook_error,
            );
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
                HookOutputMode::from_runtime_output(output_context),
            ) {
                report_nonfatal_hook_error(
                    HookOutputMode::from_runtime_output(output_context),
                    &error,
                );
            }
            return Ok(());
        }

        if shutdown.force_requested() {
            if !*force_notice_shown {
                if HookOutputMode::from_runtime_output(output_context) == HookOutputMode::Terminal {
                    eprintln!("second interrupt received, force-stopping services.");
                }
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
                HookOutputMode::from_runtime_output(output_context),
            ) {
                report_nonfatal_hook_error(
                    HookOutputMode::from_runtime_output(output_context),
                    &error,
                );
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
                HookOutputMode::from_runtime_output(output_context),
            ) {
                report_nonfatal_hook_error(
                    HookOutputMode::from_runtime_output(output_context),
                    &error,
                );
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
                        HookOutputMode::from_runtime_output(output_context),
                    ) {
                        report_nonfatal_hook_error(
                            HookOutputMode::from_runtime_output(output_context),
                            &error,
                        );
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
                        HookOutputMode::from_runtime_output(output_context),
                    ) {
                        report_nonfatal_hook_error(
                            HookOutputMode::from_runtime_output(output_context),
                            &hook_error,
                        );
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
        "instance_stopping"
        | "service_stop_timeout"
        | "action_timed_out"
        | "service_restart_skipped"
        | "watch_restart_skipped" => "WARN",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookOutputMode {
    Terminal,
    Silent,
}

impl HookOutputMode {
    fn from_runtime_output(output_context: &RuntimeOutputContext) -> Self {
        match output_context {
            RuntimeOutputContext::Plain => Self::Terminal,
            RuntimeOutputContext::Daemon | RuntimeOutputContext::Tui(_) => Self::Silent,
        }
    }
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

fn prepare_action_execution(
    action: &ResolvedActionConfig,
    context: &ActionRenderContext,
) -> AppResult<(ResolvedActionConfig, PreparedActionExecution)> {
    let prepared = context.prepare(&action.args)?;
    let mut rendered_action = action.clone();
    rendered_action.args = prepared.rendered_args.clone();
    Ok((rendered_action, prepared))
}

fn format_action_params_preview(
    action_name: &str,
    resolved_params: &BTreeMap<String, String>,
) -> String {
    let mut lines = vec![format!("action `{action_name}` resolved params:")];
    if resolved_params.is_empty() {
        lines.push("- (none)".to_owned());
        return lines.join("\n");
    }

    for (key, value) in resolved_params {
        lines.push(format!("- {key}={value}"));
    }
    lines.join("\n")
}

fn format_action_params_summary(
    action_name: &str,
    resolved_params: &BTreeMap<String, String>,
) -> String {
    if resolved_params.is_empty() {
        return format!("action `{action_name}` resolved params: (none)");
    }

    let joined = resolved_params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("action `{action_name}` resolved params: {joined}")
}

fn announce_action_params(
    project_root: Option<&Path>,
    service_name: &str,
    hook_name: &str,
    action_name: &str,
    resolved_params: &BTreeMap<String, String>,
    output_mode: HookOutputMode,
) {
    if output_mode == HookOutputMode::Terminal {
        println!(
            "{}",
            format_action_params_preview(action_name, resolved_params)
        );
    }
    if let Some(project_root) = project_root {
        emit_runtime_event(
            project_root,
            "action_params_resolved",
            Some(service_name),
            Some(hook_name),
            Some(action_name),
            format_action_params_summary(action_name, resolved_params),
        );
    }
}

fn execute_hook_actions(
    project_root: Option<&Path>,
    config_path: &Path,
    actions: &BTreeMap<String, ResolvedActionConfig>,
    service: &ResolvedServiceConfig,
    hook: HookName,
    extras: HookRuntimeExtras,
    output_mode: HookOutputMode,
) -> AppResult<()> {
    let action_names = service.hooks.actions_for(hook);
    if action_names.is_empty() {
        return Ok(());
    }

    if let Some(project_root) = project_root {
        emit_runtime_event(
            project_root,
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
    }

    for action_name in action_names {
        let action = actions.get(action_name).ok_or_else(|| {
            AppError::config_invalid(format!(
                "service `{}` references unknown action `{action_name}` in `{}`",
                service.name,
                hook.as_str()
            ))
        })?;
        if let Some(project_root) = project_root {
            emit_runtime_event(
                project_root,
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
        }

        let context = ActionRenderContext {
            project_root: config_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf(),
            config_path: config_path.to_path_buf(),
            service_name: service.name.clone(),
            action_name: action.name.clone(),
            hook_name: hook.as_str().to_owned(),
            service_cwd: service.cwd.clone(),
            service_executable: service.executable.clone(),
            service_pid: extras.service_pid.map(|pid| pid.to_string()),
            stop_reason: extras.stop_reason.clone(),
            exit_code: extras.exit_code.map(|code| code.to_string()),
            exit_status: extras.exit_status.clone(),
        };
        let (rendered_action, prepared) = prepare_action_execution(action, &context)?;
        announce_action_params(
            project_root,
            &service.name,
            hook.as_str(),
            &action.name,
            &prepared.resolved_params,
            output_mode,
        );

        match process::run_action(&rendered_action, &service.name, hook.as_str())? {
            ActionRunStatus::Succeeded => {
                if let Some(project_root) = project_root {
                    emit_runtime_event(
                        project_root,
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
            }
            ActionRunStatus::Failed { status } => {
                let error = AppError::startup_failed(format!(
                    "action `{}` for service `{}` hook `{}` exited with status {status}",
                    action.name,
                    service.name,
                    hook.as_str()
                ));
                if let Some(project_root) = project_root {
                    emit_runtime_event(
                        project_root,
                        "action_failed",
                        Some(&service.name),
                        Some(hook.as_str()),
                        Some(&action.name),
                        error.to_string(),
                    );
                    emit_runtime_event(
                        project_root,
                        "hook_failed",
                        Some(&service.name),
                        Some(hook.as_str()),
                        None,
                        error.to_string(),
                    );
                }
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
                if let Some(project_root) = project_root {
                    emit_runtime_event(
                        project_root,
                        "action_timed_out",
                        Some(&service.name),
                        Some(hook.as_str()),
                        Some(&action.name),
                        error.to_string(),
                    );
                    emit_runtime_event(
                        project_root,
                        "hook_failed",
                        Some(&service.name),
                        Some(hook.as_str()),
                        None,
                        error.to_string(),
                    );
                }
                return Err(error);
            }
        }
    }

    if let Some(project_root) = project_root {
        emit_runtime_event(
            project_root,
            "hook_finished",
            Some(&service.name),
            Some(hook.as_str()),
            None,
            format!(
                "hook `{}` finished for service `{}`",
                hook.as_str(),
                service.name
            ),
        );
    }
    Ok(())
}

fn run_hook_with_context(
    plan: &RunPlan,
    service: &ResolvedServiceConfig,
    hook: HookName,
    extras: HookRuntimeExtras,
    output_mode: HookOutputMode,
) -> AppResult<()> {
    execute_hook_actions(
        Some(&plan.project_root),
        &plan.config_path,
        &plan.actions,
        service,
        hook,
        extras,
        output_mode,
    )
}

fn run_hook_with_bundle(
    bundle: &HookBundle,
    service: &ResolvedServiceConfig,
    hook: HookName,
    extras: HookRuntimeExtras,
    output_mode: HookOutputMode,
) -> AppResult<()> {
    execute_hook_actions(
        Some(&bundle.project_root),
        &bundle.config_path,
        &bundle.actions,
        service,
        hook,
        extras,
        output_mode,
    )
}

fn report_nonfatal_hook_error(output_mode: HookOutputMode, error: &AppError) {
    if output_mode == HookOutputMode::Terminal {
        eprintln!("{error}");
    }
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
            HookOutputMode::Terminal,
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
                    HookOutputMode::Terminal,
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
                HookOutputMode::Terminal,
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
            HookOutputMode::Terminal,
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

    fn reset(&self) {
        self.signal_count.store(0, Ordering::SeqCst);
    }

    fn force_requested_now() -> Self {
        Self {
            signal_count: Arc::new(AtomicU8::new(2)),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(signal_count: u8) -> Self {
        Self {
            signal_count: Arc::new(AtomicU8::new(signal_count)),
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
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU8;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use crate::cli::{KeyValueArg, ListArgs, RunArgs};
    use crate::config::{HookName, ProjectConfig, ResolvedLogConfig};
    use crate::process::SpawnedProcess;
    use crate::runtime_state::{
        self, PlatformRuntimeState, RegistryEntry, RuntimeEvent, RuntimeState, ServiceRuntimeState,
    };

    use super::{
        HookOutputMode, HookRuntimeExtras, HookSelection, ListRequest, ManagementEntry,
        PendingWatchRestart, RuntimeOutputContext, ServiceRestartOutcome, ServiceRestartTrigger,
        ShutdownController, SingleRunRequest, WatchRuntime, build_run_plan, current_unix_secs,
        emit_runtime_event, format_action_params_preview, format_detail_fields,
        handle_runtime_exit, install_instance_logger, render_list_output, restart_service,
        run_hook_with_context, run_init, shutdown_running_services, standalone_action_context,
        start_services, summarize_instance_status, summarize_last_event, summarize_service_events,
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
        assert!(raw.contains("watch:"));
        assert!(raw.contains("debounce_ms: 500"));
        assert!(raw.contains("env:"));
        assert!(raw.contains("./logs/app.log"));
        assert!(raw.contains("stop_signal:"));
        assert!(raw.contains("autostart: true"));
        assert!(raw.contains("disabled: false"));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn list_names_include_services_actions_and_disabled_marker() {
        let dir = temp_dir("list-names");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, list_test_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let output = render_list_output(
            &config,
            &ListRequest::from_args(ListArgs {
                all: false,
                services: false,
                actions: false,
                detail: false,
                dag: false,
            }),
        );

        assert!(output.contains("services:"));
        assert!(output.contains("- api"));
        assert!(output.contains("- worker [disabled]"));
        assert!(output.contains("actions:"));
        assert!(output.contains("- cleanup [disabled]"));
        assert!(output.contains("- orphan"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn list_detail_outputs_selected_fields() {
        let dir = temp_dir("list-detail");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, list_test_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let output = render_list_output(
            &config,
            &ListRequest::from_args(ListArgs {
                all: false,
                services: false,
                actions: false,
                detail: true,
                dag: false,
            }),
        );

        assert!(output.contains("services:"));
        assert!(output.contains("stop_timeout_secs: 15"));
        assert!(output.contains("hooks:"));
        assert!(output.contains("before_start: [\"notify\"]"));
        assert!(output.contains("actions:"));
        assert!(output.contains("timeout_secs: 30"));
        assert!(output.contains("disabled: true"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn list_detail_includes_watch_config_when_present() {
        let dir = temp_dir("list-detail-watch");
        let config_path = dir.join("onekey-tasks.yaml");
        let watch_dir = dir.join("src");
        fs::create_dir_all(&watch_dir).unwrap();
        fs::write(
            &config_path,
            r#"
services:
  api:
    executable: "echo"
    watch:
      paths: ["./src"]
      debounce_ms: 250
"#,
        )
        .unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let output = render_list_output(
            &config,
            &ListRequest::from_args(ListArgs {
                all: false,
                services: false,
                actions: false,
                detail: true,
                dag: false,
            }),
        );

        assert!(output.contains("watch: paths=[\"./src\"] debounce_ms=250"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn list_dag_outputs_dependencies_hook_edges_and_standalone_actions() {
        let dir = temp_dir("list-dag");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, list_test_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let output = render_list_output(
            &config,
            &ListRequest::from_args(ListArgs {
                all: false,
                services: false,
                actions: false,
                detail: false,
                dag: true,
            }),
        );

        assert!(output.contains("service dependencies:"));
        assert!(output.contains("service: worker [disabled] --depends_on--> service: api"));
        assert!(output.contains("hook references:"));
        assert!(output.contains("service: api --hooks.before_start--> action: notify"));
        assert!(output.contains("standalone actions:"));
        assert!(output.contains("- cleanup [disabled]"));
        assert!(output.contains("- orphan"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn single_run_request_defaults_service_mode_to_no_hooks() {
        let request = SingleRunRequest::from_args(RunArgs {
            service: Some("api".to_owned()),
            action: None,
            with_all_hooks: false,
            without_hooks: false,
            hook: Vec::new(),
            args: Vec::new(),
        })
        .unwrap();

        match request.target {
            super::RunTarget::Service {
                service_name,
                hook_selection,
            } => {
                assert_eq!(service_name, "api");
                assert_eq!(hook_selection, HookSelection::None);
            }
            _ => panic!("expected service run target"),
        }
    }

    #[test]
    fn single_run_request_accepts_selected_hooks() {
        let request = SingleRunRequest::from_args(RunArgs {
            service: Some("api".to_owned()),
            action: None,
            with_all_hooks: false,
            without_hooks: false,
            hook: vec!["before_start".to_owned(), "after_stop_success".to_owned()],
            args: Vec::new(),
        })
        .unwrap();

        match request.target {
            super::RunTarget::Service { hook_selection, .. } => {
                assert!(hook_selection.allows(HookName::BeforeStart));
                assert!(hook_selection.allows(HookName::AfterStopSuccess));
                assert!(!hook_selection.allows(HookName::AfterStartFailure));
            }
            _ => panic!("expected service run target"),
        }
    }

    #[test]
    fn single_run_request_rejects_unknown_action_arg_name() {
        let error = SingleRunRequest::from_args(RunArgs {
            service: None,
            action: Some("notify".to_owned()),
            with_all_hooks: false,
            without_hooks: false,
            hook: Vec::new(),
            args: vec![KeyValueArg {
                key: "servie_name".to_owned(),
                value: "api".to_owned(),
            }],
        })
        .unwrap_err();

        assert!(error.to_string().contains("unknown standalone action arg"));
    }

    #[test]
    fn standalone_action_context_infers_service_fields_from_service_name() {
        let dir = temp_dir("standalone-context");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(
            &config_path,
            r#"
actions:
  notify:
    executable: "echo"
services:
  api:
    executable: "cargo"
    cwd: ./backend
"#,
        )
        .unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let overrides = BTreeMap::from([("service_name".to_owned(), "api".to_owned())]);
        let context =
            standalone_action_context(&config, &config_path, &dir, "notify", &overrides).unwrap();

        assert_eq!(context.service_name, "api");
        assert!(context.service_cwd.ends_with("backend"));
        assert_eq!(context.service_executable, "cargo");
        assert_eq!(context.hook_name, "manual");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn action_params_preview_lists_each_used_param() {
        let preview = format_action_params_preview(
            "notify",
            &BTreeMap::from([
                ("hook_name".to_owned(), "before_start".to_owned()),
                ("service_name".to_owned(), "api".to_owned()),
            ]),
        );

        assert!(preview.contains("action `notify` resolved params:"));
        assert!(preview.contains("- hook_name=before_start"));
        assert!(preview.contains("- service_name=api"));
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
            HookOutputMode::Terminal,
        );

        assert!(result.is_ok(), "{result:?}");
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"hook_started\""));
        assert!(events_raw.contains("\"event_type\":\"action_params_resolved\""));
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
            HookOutputMode::Terminal,
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
            HookOutputMode::Terminal,
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

        handle_runtime_exit(&plan, &mut spawned, status, &RuntimeOutputContext::Plain).unwrap();

        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"service_runtime_exit_unexpected\""));
        assert!(events_raw.contains("\"hook_name\":\"after_runtime_exit_unexpected\""));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn watch_restart_restarts_service_and_runs_stop_hook() {
        let dir = temp_dir("watch-restart");
        let config_path = dir.join("onekey-tasks.yaml");
        let trigger_path = dir.join("trigger.txt");
        fs::write(&trigger_path, "v1\n").unwrap();
        fs::write(&config_path, watch_restart_config()).unwrap();
        fs::create_dir_all(dir.join(runtime_state::RUNTIME_DIR)).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let output_context = RuntimeOutputContext::Plain;
        let mut runtime_state = RuntimeState::new(dir.clone(), config_path.clone(), None);
        let mut running = start_services(&plan, &mut runtime_state, &output_context).unwrap();
        let mut watch_runtime = WatchRuntime::start(&plan).unwrap().unwrap();
        let shutdown = test_shutdown_controller();
        let original_pid = running[0].state.pid;

        thread::sleep(Duration::from_millis(300));
        fs::write(&trigger_path, "v2\n").unwrap();
        wait_for_watch_tick(
            &mut watch_runtime,
            &plan,
            &mut running,
            &mut runtime_state,
            &shutdown,
            &output_context,
            |running| running[0].state.pid != original_pid,
        );

        assert_ne!(running[0].state.pid, original_pid);
        let stop_reasons = fs::read_to_string(dir.join("stop-reasons.log")).unwrap();
        assert!(stop_reasons.lines().any(|line| line == "watch_restart"));
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"service_restart_requested\""));
        assert!(events_raw.contains("\"event_type\":\"service_restart_succeeded\""));
        assert!(events_raw.contains("\"detail\":\"trigger=\\\"watch\\\""));

        drop(watch_runtime);
        shutdown_running_services(
            &mut running,
            &shutdown,
            &plan,
            "test_cleanup",
            &output_context,
        )
        .unwrap();
        let _ = runtime_state::cleanup_runtime_files(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn watch_ignores_runtime_and_log_file_changes() {
        let dir = temp_dir("watch-ignore");
        let config_path = dir.join("onekey-tasks.yaml");
        let watched_file = dir.join("src.txt");
        fs::write(&watched_file, "v1\n").unwrap();
        fs::write(&config_path, watch_ignore_config()).unwrap();
        fs::create_dir_all(dir.join(runtime_state::RUNTIME_DIR)).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let output_context = RuntimeOutputContext::Plain;
        let mut runtime_state = RuntimeState::new(
            dir.clone(),
            config_path.clone(),
            plan.instance_log.as_ref().map(|log| log.file.clone()),
        );
        let mut running = start_services(&plan, &mut runtime_state, &output_context).unwrap();
        let mut watch_runtime = WatchRuntime::start(&plan).unwrap().unwrap();
        let shutdown = test_shutdown_controller();
        let original_pid = running[0].state.pid;

        thread::sleep(Duration::from_millis(300));
        emit_runtime_event(
            &dir,
            "manual_probe",
            Some("api"),
            None,
            None,
            "testing runtime event ignore".to_owned(),
        );
        fs::create_dir_all(dir.join("logs")).unwrap();
        fs::write(dir.join("logs").join("api.log"), "service-log\n").unwrap();
        fs::write(dir.join("logs").join("onekey-run.log"), "instance-log\n").unwrap();

        for _ in 0..10 {
            watch_runtime
                .tick(
                    &plan,
                    &mut running,
                    &mut runtime_state,
                    &shutdown,
                    &output_context,
                )
                .unwrap();
            thread::sleep(Duration::from_millis(120));
        }
        assert_eq!(running[0].state.pid, original_pid);

        fs::write(&watched_file, "v2\n").unwrap();
        wait_for_watch_tick(
            &mut watch_runtime,
            &plan,
            &mut running,
            &mut runtime_state,
            &shutdown,
            &output_context,
            |running| running[0].state.pid != original_pid,
        );
        assert_ne!(running[0].state.pid, original_pid);

        drop(watch_runtime);
        shutdown_running_services(
            &mut running,
            &shutdown,
            &plan,
            "test_cleanup",
            &output_context,
        )
        .unwrap();
        let _ = runtime_state::cleanup_runtime_files(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tui_restart_restarts_running_service_and_emits_events() {
        let dir = temp_dir("tui-restart-running");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(dir.join("trigger.txt"), "v1\n").unwrap();
        fs::write(&config_path, watch_restart_config()).unwrap();
        fs::create_dir_all(dir.join(runtime_state::RUNTIME_DIR)).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let output_context = RuntimeOutputContext::Plain;
        let mut runtime_state = RuntimeState::new(dir.clone(), config_path.clone(), None);
        let mut running = start_services(&plan, &mut runtime_state, &output_context).unwrap();
        let shutdown = test_shutdown_controller();
        let original_pid = running[0].state.pid;

        let outcome = restart_service(
            &plan,
            "api",
            ServiceRestartTrigger::Tui,
            &mut running,
            &mut runtime_state,
            &shutdown,
            &output_context,
        )
        .unwrap();

        assert_eq!(outcome, ServiceRestartOutcome::Restarted);
        assert_ne!(running[0].state.pid, original_pid);
        let stop_reasons = fs::read_to_string(dir.join("stop-reasons.log")).unwrap();
        assert!(stop_reasons.lines().any(|line| line == "tui_restart"));
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"service_restart_requested\""));
        assert!(events_raw.contains("\"event_type\":\"service_restart_succeeded\""));
        assert!(events_raw.contains("trigger=\\\"tui\\\""));
        assert!(events_raw.contains("key=\\\"R\\\""));

        shutdown_running_services(
            &mut running,
            &shutdown,
            &plan,
            "test_cleanup",
            &output_context,
        )
        .unwrap();
        let _ = runtime_state::cleanup_runtime_files(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tui_restart_starts_service_when_not_running() {
        let dir = temp_dir("tui-restart-stopped");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(dir.join("trigger.txt"), "v1\n").unwrap();
        fs::write(&config_path, watch_restart_config()).unwrap();
        fs::create_dir_all(dir.join(runtime_state::RUNTIME_DIR)).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let output_context = RuntimeOutputContext::Plain;
        let mut runtime_state = RuntimeState::new(dir.clone(), config_path.clone(), None);
        runtime_state::write_state(&dir, &runtime_state).unwrap();
        let mut running = Vec::new();
        let shutdown = test_shutdown_controller();

        let outcome = restart_service(
            &plan,
            "api",
            ServiceRestartTrigger::Tui,
            &mut running,
            &mut runtime_state,
            &shutdown,
            &output_context,
        )
        .unwrap();

        assert_eq!(outcome, ServiceRestartOutcome::Restarted);
        assert_eq!(running.len(), 1);
        assert_eq!(runtime_state.services.len(), 1);
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"service_restart_succeeded\""));

        shutdown_running_services(
            &mut running,
            &shutdown,
            &plan,
            "test_cleanup",
            &output_context,
        )
        .unwrap();
        let _ = runtime_state::cleanup_runtime_files(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tui_restart_is_skipped_when_shutdown_requested() {
        let dir = temp_dir("tui-restart-shutdown");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(dir.join("trigger.txt"), "v1\n").unwrap();
        fs::write(&config_path, watch_restart_config()).unwrap();
        fs::create_dir_all(dir.join(runtime_state::RUNTIME_DIR)).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let output_context = RuntimeOutputContext::Plain;
        let mut runtime_state = RuntimeState::new(dir.clone(), config_path.clone(), None);
        runtime_state::write_state(&dir, &runtime_state).unwrap();
        let mut running = Vec::new();
        let shutdown = shutdown_requested_controller();

        let outcome = restart_service(
            &plan,
            "api",
            ServiceRestartTrigger::Tui,
            &mut running,
            &mut runtime_state,
            &shutdown,
            &output_context,
        )
        .unwrap();

        assert_eq!(
            outcome,
            ServiceRestartOutcome::Skipped {
                detail: "shutdown in progress".to_owned(),
            }
        );
        assert!(running.is_empty());
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"service_restart_skipped\""));
        assert!(events_raw.contains("reason=\\\"shutdown\\\""));

        let _ = runtime_state::cleanup_runtime_files(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tui_restart_start_failure_emits_skip_event() {
        let dir = temp_dir("tui-restart-start-fail");
        let config_path = dir.join("onekey-tasks.yaml");
        fs::write(&config_path, broken_service_config()).unwrap();
        fs::create_dir_all(dir.join(runtime_state::RUNTIME_DIR)).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let output_context = RuntimeOutputContext::Plain;
        let mut runtime_state = RuntimeState::new(dir.clone(), config_path.clone(), None);
        runtime_state::write_state(&dir, &runtime_state).unwrap();
        let mut running = Vec::new();
        let shutdown = test_shutdown_controller();

        let outcome = restart_service(
            &plan,
            "api",
            ServiceRestartTrigger::Tui,
            &mut running,
            &mut runtime_state,
            &shutdown,
            &output_context,
        )
        .unwrap();

        match outcome {
            ServiceRestartOutcome::Restarted => {
                panic!("expected restart to be skipped after start failure")
            }
            ServiceRestartOutcome::Skipped { detail } => {
                assert!(detail.contains("failed to start service `api`"));
            }
        }
        let events_raw = fs::read_to_string(runtime_state::events_path(&dir)).unwrap();
        assert!(events_raw.contains("\"event_type\":\"service_restart_skipped\""));
        assert!(events_raw.contains("reason=\\\"start_failed\\\""));

        let _ = runtime_state::cleanup_runtime_files(&dir);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_pending_restart_removes_only_selected_service() {
        let dir = temp_dir("clear-pending");
        let config_path = dir.join("onekey-tasks.yaml");
        let trigger_path = dir.join("trigger.txt");
        fs::write(&trigger_path, "v1\n").unwrap();
        fs::write(&config_path, watch_restart_config()).unwrap();

        let config = ProjectConfig::load(&config_path).unwrap();
        let plan = build_run_plan(&config, &config_path, &[]).unwrap();
        let mut watch_runtime = WatchRuntime::start(&plan).unwrap().unwrap();

        watch_runtime.pending.insert(
            "api".to_owned(),
            PendingWatchRestart {
                changed_path: trigger_path,
                ready_at: Instant::now() + Duration::from_secs(1),
            },
        );
        watch_runtime.pending.insert(
            "other".to_owned(),
            PendingWatchRestart {
                changed_path: dir.join("other.txt"),
                ready_at: Instant::now() + Duration::from_secs(1),
            },
        );

        watch_runtime.clear_pending("api");

        assert!(!watch_runtime.pending.contains_key("api"));
        assert!(watch_runtime.pending.contains_key("other"));

        drop(watch_runtime);
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

    fn watch_restart_config() -> String {
        format!(
            r#"
actions:
  record-stop:
    executable: "{action_executable}"
    args: {action_args}
    cwd: .
services:
  api:
    executable: "{service_executable}"
    args: {service_args}
    cwd: .
    log:
      file: ./logs/api.log
      append: true
    watch:
      paths: ["./trigger.txt"]
      debounce_ms: 150
    hooks:
      before_stop: ["record-stop"]
"#,
            action_executable = watch_action_executable(),
            action_args = yaml_string_array(&watch_record_stop_args()),
            service_executable = watch_service_executable(),
            service_args = yaml_string_array(&watch_service_args()),
        )
    }

    fn watch_ignore_config() -> String {
        format!(
            r#"
log:
  file: ./logs/onekey-run.log
  append: true
services:
  api:
    executable: "{service_executable}"
    args: {service_args}
    cwd: .
    log:
      file: ./logs/api.log
      append: true
    watch:
      paths: ["./"]
      debounce_ms: 150
"#,
            service_executable = watch_service_executable(),
            service_args = yaml_string_array(&watch_service_args()),
        )
    }

    fn test_shutdown_controller() -> ShutdownController {
        ShutdownController {
            signal_count: Arc::new(AtomicU8::new(0)),
        }
    }

    fn shutdown_requested_controller() -> ShutdownController {
        ShutdownController {
            signal_count: Arc::new(AtomicU8::new(1)),
        }
    }

    fn wait_for_watch_tick<F>(
        watch_runtime: &mut WatchRuntime,
        plan: &super::RunPlan,
        running: &mut Vec<SpawnedProcess>,
        runtime_state: &mut RuntimeState,
        shutdown: &ShutdownController,
        output_context: &RuntimeOutputContext,
        predicate: F,
    ) where
        F: Fn(&[SpawnedProcess]) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            watch_runtime
                .tick(plan, running, runtime_state, shutdown, output_context)
                .unwrap();
            if predicate(running) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }

        panic!("timed out waiting for watch-triggered state change");
    }

    #[cfg(unix)]
    fn watch_service_executable() -> &'static str {
        "sh"
    }

    #[cfg(windows)]
    fn watch_service_executable() -> &'static str {
        "cmd"
    }

    #[cfg(unix)]
    fn watch_service_args() -> Vec<String> {
        vec![
            "-c".to_owned(),
            "trap 'exit 0' TERM INT; while true; do sleep 1; done".to_owned(),
        ]
    }

    #[cfg(windows)]
    fn watch_service_args() -> Vec<String> {
        vec![
            "/C".to_owned(),
            "powershell -NoProfile -Command \"while ($true) { Start-Sleep -Seconds 1 }\""
                .to_owned(),
        ]
    }

    fn broken_service_config() -> &'static str {
        r#"
services:
  api:
    executable: "__onekey_missing_executable__"
    args: []
    cwd: .
"#
    }

    #[cfg(unix)]
    fn watch_action_executable() -> &'static str {
        "sh"
    }

    #[cfg(windows)]
    fn watch_action_executable() -> &'static str {
        "cmd"
    }

    #[cfg(unix)]
    fn watch_record_stop_args() -> Vec<String> {
        vec![
            "-c".to_owned(),
            "printf '%s\\n' \"${stop_reason}\" >> ./stop-reasons.log".to_owned(),
        ]
    }

    #[cfg(windows)]
    fn watch_record_stop_args() -> Vec<String> {
        vec![
            "/C".to_owned(),
            "echo ${stop_reason}>> stop-reasons.log".to_owned(),
        ]
    }

    fn yaml_string_array(values: &[String]) -> String {
        let rendered = values
            .iter()
            .map(|value| serde_json::to_string(value).unwrap())
            .collect::<Vec<_>>()
            .join(", ");
        format!("[{rendered}]")
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

    fn list_test_config() -> &'static str {
        r#"
actions:
  cleanup:
    executable: "echo"
    disabled: true
  notify:
    executable: "echo"
    args: ["service-started"]
    cwd: .
    timeout_secs: 30
  orphan:
    executable: "echo"
services:
  api:
    executable: "echo"
    args: ["api"]
    cwd: .
    env:
      RUST_LOG: info
    stop_timeout_secs: 15
    autostart: true
    log:
      file: ./logs/api.log
      append: true
    hooks:
      before_start: ["notify"]
  worker:
    executable: "echo"
    depends_on: ["api"]
    disabled: true
"#
    }
}
