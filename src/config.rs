use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[cfg(windows)]
use std::ffi::OsString;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default, skip_serializing_if = "DefaultsConfig::is_empty")]
    pub defaults: DefaultsConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LogConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, ActionConfig>,
    pub services: BTreeMap<String, ServiceConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub restart: Option<RestartPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    No,
    OnFailure,
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub executable: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub restart: Option<RestartPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LogConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<ServiceWatchConfig>,
    #[serde(default, skip_serializing_if = "ServiceHooksConfig::is_empty")]
    pub hooks: ServiceHooksConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceWatchConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionConfig {
    pub executable: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceHooksConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before_start: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_start_success: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_start_failure: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before_stop: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_stop_success: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_stop_timeout: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_stop_failure: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after_runtime_exit_unexpected: Vec<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub enum HookName {
    BeforeStart,
    AfterStartSuccess,
    AfterStartFailure,
    BeforeStop,
    AfterStopSuccess,
    AfterStopTimeout,
    AfterStopFailure,
    AfterRuntimeExitUnexpected,
}

#[derive(Debug, Clone)]
pub struct ResolvedServiceConfig {
    pub name: String,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    #[allow(dead_code)]
    pub depends_on: Vec<String>,
    pub hooks: ServiceHooksConfig,
    pub stop_signal: Option<String>,
    pub stop_timeout_secs: u64,
    pub log: Option<ResolvedLogConfig>,
    pub watch: Option<ResolvedServiceWatchConfig>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ResolvedServiceWatchConfig {
    pub paths: Vec<PathBuf>,
    pub debounce_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ResolvedActionConfig {
    pub name: String,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ActionRenderContext {
    pub project_root: PathBuf,
    pub config_path: PathBuf,
    pub service_name: String,
    pub action_name: String,
    pub hook_name: String,
    pub service_cwd: PathBuf,
    pub service_executable: String,
    pub service_pid: Option<String>,
    pub stop_reason: Option<String>,
    pub exit_code: Option<String>,
    pub exit_status: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PreparedActionExecution {
    pub rendered_args: Vec<String>,
    pub resolved_params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    pub file: PathBuf,
    #[serde(default = "default_log_append")]
    pub append: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overflow_strategy: Option<LogOverflowStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate_file_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ResolvedLogConfig {
    pub file: PathBuf,
    pub append: bool,
    pub max_file_bytes: Option<u64>,
    pub overflow_strategy: Option<LogOverflowStrategy>,
    pub rotate_file_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LogOverflowStrategy {
    Rotate,
    Archive,
}

impl ProjectConfig {
    pub fn preset_minimal() -> Self {
        if cfg!(windows) {
            return Self::preset_minimal_windows();
        }

        Self::preset_minimal_unix()
    }

    pub fn preset_full() -> Self {
        if cfg!(windows) {
            return Self::preset_full_windows();
        }

        Self::preset_full_unix()
    }

    pub fn preset_minimal_unix() -> Self {
        let mut services = BTreeMap::new();
        services.insert(
            "app".to_owned(),
            ServiceConfig {
                executable: "sleep".to_owned(),
                args: vec!["30".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                depends_on: Vec::new(),
                restart: None,
                stop_signal: None,
                stop_timeout_secs: None,
                autostart: None,
                disabled: None,
                log: Some(default_rotate_log("./logs/app.log")),
                watch: None,
                hooks: ServiceHooksConfig::default(),
            },
        );
        services.insert(
            "worker".to_owned(),
            ServiceConfig {
                executable: "sleep".to_owned(),
                args: vec!["30".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                depends_on: vec!["app".to_owned()],
                restart: None,
                stop_signal: None,
                stop_timeout_secs: None,
                autostart: None,
                disabled: None,
                log: Some(default_rotate_log("./logs/worker.log")),
                watch: None,
                hooks: ServiceHooksConfig::default(),
            },
        );

        Self {
            defaults: DefaultsConfig {
                stop_timeout_secs: Some(10),
                restart: None,
            },
            log: Some(default_rotate_log("./logs/onekey-run.log")),
            actions: BTreeMap::new(),
            services,
        }
    }

    pub fn preset_minimal_windows() -> Self {
        let mut services = BTreeMap::new();
        services.insert(
            "app".to_owned(),
            ServiceConfig {
                executable: "cmd".to_owned(),
                args: vec!["/C".to_owned(), "timeout /T 30 /NOBREAK >NUL".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                depends_on: Vec::new(),
                restart: None,
                stop_signal: None,
                stop_timeout_secs: None,
                autostart: None,
                disabled: None,
                log: Some(default_rotate_log("./logs/app.log")),
                watch: None,
                hooks: ServiceHooksConfig::default(),
            },
        );
        services.insert(
            "worker".to_owned(),
            ServiceConfig {
                executable: "cmd".to_owned(),
                args: vec!["/C".to_owned(), "timeout /T 30 /NOBREAK >NUL".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                depends_on: vec!["app".to_owned()],
                restart: None,
                stop_signal: None,
                stop_timeout_secs: None,
                autostart: None,
                disabled: None,
                log: Some(default_rotate_log("./logs/worker.log")),
                watch: None,
                hooks: ServiceHooksConfig::default(),
            },
        );

        Self {
            defaults: DefaultsConfig {
                stop_timeout_secs: Some(10),
                restart: None,
            },
            log: Some(default_rotate_log("./logs/onekey-run.log")),
            actions: BTreeMap::new(),
            services,
        }
    }

    pub fn preset_full_unix() -> Self {
        let mut actions = BTreeMap::new();
        actions.insert(
            "prepare-app".to_owned(),
            ActionConfig {
                executable: "sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "echo preparing ${service_name} in ${service_cwd}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );
        actions.insert(
            "notify-up".to_owned(),
            ActionConfig {
                executable: "sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "echo ${service_name} started with pid ${service_pid}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );
        actions.insert(
            "notify-stop".to_owned(),
            ActionConfig {
                executable: "sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "echo stopping ${service_name} because ${stop_reason}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );
        actions.insert(
            "notify-exit".to_owned(),
            ActionConfig {
                executable: "sh".to_owned(),
                args: vec![
                    "-c".to_owned(),
                    "echo ${service_name} exited unexpectedly with ${exit_status}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );

        let mut app_env = BTreeMap::new();
        app_env.insert("RUST_LOG".to_owned(), "info".to_owned());

        let mut services = BTreeMap::new();
        services.insert(
            "app".to_owned(),
            ServiceConfig {
                executable: "sleep".to_owned(),
                args: vec!["30".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: app_env,
                depends_on: Vec::new(),
                restart: Some(RestartPolicy::No),
                stop_signal: Some("term".to_owned()),
                stop_timeout_secs: Some(10),
                autostart: Some(true),
                disabled: Some(false),
                log: Some(default_rotate_log("./logs/app.log")),
                watch: Some(ServiceWatchConfig {
                    paths: vec![PathBuf::from(".")],
                    debounce_ms: Some(500),
                }),
                hooks: ServiceHooksConfig {
                    before_start: vec!["prepare-app".to_owned()],
                    after_start_success: vec!["notify-up".to_owned()],
                    after_start_failure: Vec::new(),
                    before_stop: vec!["notify-stop".to_owned()],
                    after_stop_success: Vec::new(),
                    after_stop_timeout: Vec::new(),
                    after_stop_failure: Vec::new(),
                    after_runtime_exit_unexpected: vec!["notify-exit".to_owned()],
                },
            },
        );
        services.insert(
            "worker".to_owned(),
            ServiceConfig {
                executable: "sleep".to_owned(),
                args: vec!["30".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                depends_on: vec!["app".to_owned()],
                restart: Some(RestartPolicy::OnFailure),
                stop_signal: Some("term".to_owned()),
                stop_timeout_secs: Some(10),
                autostart: Some(true),
                disabled: Some(false),
                log: Some(default_rotate_log("./logs/worker.log")),
                watch: None,
                hooks: ServiceHooksConfig {
                    before_start: vec!["prepare-app".to_owned()],
                    after_start_success: Vec::new(),
                    after_start_failure: Vec::new(),
                    before_stop: Vec::new(),
                    after_stop_success: Vec::new(),
                    after_stop_timeout: Vec::new(),
                    after_stop_failure: Vec::new(),
                    after_runtime_exit_unexpected: vec!["notify-exit".to_owned()],
                },
            },
        );

        Self {
            defaults: DefaultsConfig {
                stop_timeout_secs: Some(10),
                restart: Some(RestartPolicy::No),
            },
            log: Some(default_rotate_log("./logs/onekey-run.log")),
            actions,
            services,
        }
    }

    pub fn preset_full_windows() -> Self {
        let mut actions = BTreeMap::new();
        actions.insert(
            "prepare-app".to_owned(),
            ActionConfig {
                executable: "cmd".to_owned(),
                args: vec![
                    "/C".to_owned(),
                    "echo preparing ${service_name} in ${service_cwd}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );
        actions.insert(
            "notify-up".to_owned(),
            ActionConfig {
                executable: "cmd".to_owned(),
                args: vec![
                    "/C".to_owned(),
                    "echo ${service_name} started with pid ${service_pid}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );
        actions.insert(
            "notify-stop".to_owned(),
            ActionConfig {
                executable: "cmd".to_owned(),
                args: vec![
                    "/C".to_owned(),
                    "echo stopping ${service_name} because ${stop_reason}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );
        actions.insert(
            "notify-exit".to_owned(),
            ActionConfig {
                executable: "cmd".to_owned(),
                args: vec![
                    "/C".to_owned(),
                    "echo ${service_name} exited unexpectedly with ${exit_status}".to_owned(),
                ],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                timeout_secs: Some(10),
                disabled: Some(false),
            },
        );

        let mut app_env = BTreeMap::new();
        app_env.insert("RUST_LOG".to_owned(), "info".to_owned());

        let mut services = BTreeMap::new();
        services.insert(
            "app".to_owned(),
            ServiceConfig {
                executable: "cmd".to_owned(),
                args: vec!["/C".to_owned(), "timeout /T 30 /NOBREAK >NUL".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: app_env,
                depends_on: Vec::new(),
                restart: Some(RestartPolicy::No),
                stop_signal: Some("term".to_owned()),
                stop_timeout_secs: Some(10),
                autostart: Some(true),
                disabled: Some(false),
                log: Some(default_rotate_log("./logs/app.log")),
                watch: Some(ServiceWatchConfig {
                    paths: vec![PathBuf::from(".")],
                    debounce_ms: Some(500),
                }),
                hooks: ServiceHooksConfig {
                    before_start: vec!["prepare-app".to_owned()],
                    after_start_success: vec!["notify-up".to_owned()],
                    after_start_failure: Vec::new(),
                    before_stop: vec!["notify-stop".to_owned()],
                    after_stop_success: Vec::new(),
                    after_stop_timeout: Vec::new(),
                    after_stop_failure: Vec::new(),
                    after_runtime_exit_unexpected: vec!["notify-exit".to_owned()],
                },
            },
        );
        services.insert(
            "worker".to_owned(),
            ServiceConfig {
                executable: "cmd".to_owned(),
                args: vec!["/C".to_owned(), "timeout /T 30 /NOBREAK >NUL".to_owned()],
                cwd: Some(PathBuf::from(".")),
                env: BTreeMap::new(),
                depends_on: vec!["app".to_owned()],
                restart: Some(RestartPolicy::OnFailure),
                stop_signal: Some("term".to_owned()),
                stop_timeout_secs: Some(10),
                autostart: Some(true),
                disabled: Some(false),
                log: Some(default_rotate_log("./logs/worker.log")),
                watch: None,
                hooks: ServiceHooksConfig {
                    before_start: vec!["prepare-app".to_owned()],
                    after_start_success: Vec::new(),
                    after_start_failure: Vec::new(),
                    before_stop: Vec::new(),
                    after_stop_success: Vec::new(),
                    after_stop_timeout: Vec::new(),
                    after_stop_failure: Vec::new(),
                    after_runtime_exit_unexpected: vec!["notify-exit".to_owned()],
                },
            },
        );

        Self {
            defaults: DefaultsConfig {
                stop_timeout_secs: Some(10),
                restart: Some(RestartPolicy::No),
            },
            log: Some(default_rotate_log("./logs/onekey-run.log")),
            actions,
            services,
        }
    }

    pub fn to_yaml_string(&self) -> AppResult<String> {
        serde_yaml::to_string(self)
            .map_err(|error| AppError::config_invalid(format!("failed to render YAML: {error}")))
    }

    pub fn load(path: &Path) -> AppResult<Self> {
        let raw = fs::read_to_string(path).map_err(|error| AppError::config_io(path, error))?;
        let config = serde_yaml::from_str::<Self>(&raw).map_err(|error| {
            AppError::config_invalid(format!(
                "failed to parse configuration at {}: {error}",
                path.display()
            ))
        })?;
        config.validate(path)?;
        Ok(config)
    }

    pub fn validate(&self, path: &Path) -> AppResult<()> {
        let mut errors = Vec::new();
        let config_root = path.parent().unwrap_or_else(|| Path::new("."));

        if self.services.is_empty() {
            errors.push(format!(
                "configuration at {} must define at least one service",
                path.display()
            ));
        }

        if let Some(log) = &self.log {
            if log.file.as_os_str().is_empty() {
                errors.push("top-level log.file must not be empty".to_owned());
            }

            if let Err(error) = validate_log_config("top-level", log) {
                errors.push(error.to_string());
            }
        }

        for (name, action) in &self.actions {
            if !is_valid_action_name(name) {
                errors.push(format!(
                    "action name `{name}` is invalid; use letters, digits, `_`, or `-`"
                ));
            }

            if action.executable.trim().is_empty() {
                errors.push(format!(
                    "action `{name}` must define a non-empty executable"
                ));
            }

            if let Some(timeout_secs) = action.timeout_secs
                && timeout_secs == 0
            {
                errors.push(format!(
                    "action `{name}` timeout_secs must be greater than 0"
                ));
            }

            if let Err(error) = validate_placeholders(&action.args) {
                errors.push(format!("action `{name}` args {error}"));
            }
        }

        for (name, service) in &self.services {
            if !is_valid_service_name(name) {
                errors.push(format!(
                    "service name `{name}` is invalid; use lowercase letters, digits, `_`, or `-`"
                ));
            }

            if service.executable.trim().is_empty() {
                errors.push(format!(
                    "service `{name}` must define a non-empty executable"
                ));
            }

            if let Some(log) = &service.log
                && log.file.as_os_str().is_empty()
            {
                errors.push(format!("service `{name}` log.file must not be empty"));
            }

            if let Some(log) = &service.log
                && let Err(error) = validate_log_config(&format!("service `{name}`"), log)
            {
                errors.push(error.to_string());
            }

            if let Some(watch) = &service.watch
                && let Err(error) =
                    validate_service_watch_config(&format!("service `{name}`"), watch, config_root)
            {
                errors.push(error.to_string());
            }

            for dependency in &service.depends_on {
                if !self.services.contains_key(dependency) {
                    errors.push(format!(
                        "service `{name}` depends on unknown service `{dependency}`"
                    ));
                }
            }

            for (hook, action_name) in service.hooks.references() {
                let Some(action) = self.actions.get(action_name) else {
                    errors.push(format!(
                        "service `{name}` hook `{}` references unknown action `{action_name}`",
                        hook.as_str()
                    ));
                    continue;
                };

                if action.is_disabled() {
                    errors.push(format!(
                        "service `{name}` hook `{}` references disabled action `{action_name}`",
                        hook.as_str()
                    ));
                    continue;
                }

                match placeholder_names(&action.args) {
                    Ok(names) => {
                        for placeholder in names {
                            if !hook.supports_placeholder(&placeholder) {
                                errors.push(format!(
                                    "service `{name}` hook `{}` action `{action_name}` cannot use placeholder `${{{placeholder}}}`",
                                    hook.as_str()
                                ));
                            }
                        }
                    }
                    Err(error) => errors.push(format!("action `{action_name}` args {error}")),
                }
            }
        }

        if let Some(top_level_log) = self
            .log
            .as_ref()
            .map(|log| resolve_path(config_root, &log.file))
        {
            for (name, service) in &self.services {
                if let Some(service_log) = &service.log {
                    let service_log_path = resolve_path(config_root, &service_log.file);
                    if service_log_path == top_level_log {
                        errors.push(format!(
                            "top-level log.file conflicts with service `{name}` log.file: {}",
                            service_log_path.display()
                        ));
                    }
                }
            }
        }

        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for name in self.services.keys() {
            if let Err(error) = self.validate_acyclic(name, &mut visiting, &mut visited) {
                errors.push(error.to_string());
                break;
            }
        }

        if errors.is_empty() {
            return Ok(());
        }

        Err(AppError::config_invalid(join_config_errors(errors)))
    }

    fn validate_acyclic(
        &self,
        name: &str,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> AppResult<()> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_owned()) {
            return Err(AppError::config_invalid(format!(
                "dependency cycle detected around service `{name}`"
            )));
        }

        let service = self
            .services
            .get(name)
            .expect("validated service lookup should succeed");
        for dependency in &service.depends_on {
            self.validate_acyclic(dependency, visiting, visited)?;
        }

        visiting.remove(name);
        visited.insert(name.to_owned());
        Ok(())
    }

    pub fn resolve_service(
        &self,
        name: &str,
        project_root: &Path,
    ) -> AppResult<ResolvedServiceConfig> {
        let service = self.services.get(name).ok_or_else(|| {
            AppError::config_invalid(format!("service `{name}` not found in configuration"))
        })?;

        let cwd = match &service.cwd {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => project_root.join(path),
            None => project_root.to_path_buf(),
        };

        if service.is_disabled() {
            return Err(AppError::config_invalid(format!(
                "service `{name}` is disabled and cannot be started"
            )));
        }

        Ok(ResolvedServiceConfig {
            name: name.to_owned(),
            executable: service.executable.clone(),
            args: service.args.clone(),
            cwd,
            env: service.env.clone(),
            depends_on: service.depends_on.clone(),
            hooks: service.hooks.clone(),
            stop_signal: service.stop_signal.clone(),
            stop_timeout_secs: service
                .stop_timeout_secs
                .or(self.defaults.stop_timeout_secs)
                .unwrap_or(10),
            log: service
                .log
                .as_ref()
                .map(|log| resolve_log_config(project_root, log)),
            watch: service
                .watch
                .as_ref()
                .map(|watch| resolve_watch_config(project_root, watch)),
        })
    }

    pub fn resolve_actions(
        &self,
        project_root: &Path,
    ) -> AppResult<BTreeMap<String, ResolvedActionConfig>> {
        let mut resolved = BTreeMap::new();
        for (name, action) in &self.actions {
            if action.is_disabled() {
                continue;
            }

            let cwd = match &action.cwd {
                Some(path) if path.is_absolute() => path.clone(),
                Some(path) => project_root.join(path),
                None => project_root.to_path_buf(),
            };

            resolved.insert(
                name.clone(),
                ResolvedActionConfig {
                    name: name.clone(),
                    executable: action.executable.clone(),
                    args: action.args.clone(),
                    cwd,
                    env: action.env.clone(),
                    timeout_secs: action.timeout_secs,
                },
            );
        }
        Ok(resolved)
    }

    pub fn resolve_project_log(&self, project_root: &Path) -> Option<ResolvedLogConfig> {
        self.log
            .as_ref()
            .map(|log| resolve_log_config(project_root, log))
    }

    pub fn should_autostart(&self, name: &str) -> bool {
        self.services
            .get(name)
            .map(|service| !service.is_disabled() && service.autostart.unwrap_or(true))
            .unwrap_or(false)
    }

    pub fn executable_exists(&self, name: &str, project_root: &Path) -> AppResult<()> {
        let resolved = self.resolve_service(name, project_root)?;
        if executable_exists(&resolved.executable, &resolved.cwd) {
            return Ok(());
        }

        Err(AppError::config_invalid(format!(
            "service `{name}` executable `{}` was not found on PATH or as a local file",
            resolved.executable
        )))
    }

    pub fn action_executable_exists(&self, name: &str, project_root: &Path) -> AppResult<()> {
        let action = self.actions.get(name).ok_or_else(|| {
            AppError::config_invalid(format!("action `{name}` not found in configuration"))
        })?;

        if action.is_disabled() {
            return Ok(());
        }

        let cwd = match &action.cwd {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => project_root.join(path),
            None => project_root.to_path_buf(),
        };

        if executable_exists(&action.executable, &cwd) {
            return Ok(());
        }

        Err(AppError::config_invalid(format!(
            "action `{name}` executable `{}` was not found on PATH or as a local file",
            action.executable
        )))
    }
}

impl ServiceConfig {
    fn is_disabled(&self) -> bool {
        self.disabled.unwrap_or(false)
    }
}

impl ActionConfig {
    fn is_disabled(&self) -> bool {
        self.disabled.unwrap_or(false)
    }
}

impl ServiceHooksConfig {
    fn is_empty(&self) -> bool {
        self.before_start.is_empty()
            && self.after_start_success.is_empty()
            && self.after_start_failure.is_empty()
            && self.before_stop.is_empty()
            && self.after_stop_success.is_empty()
            && self.after_stop_timeout.is_empty()
            && self.after_stop_failure.is_empty()
            && self.after_runtime_exit_unexpected.is_empty()
    }

    pub fn actions_for(&self, hook: HookName) -> &[String] {
        match hook {
            HookName::BeforeStart => &self.before_start,
            HookName::AfterStartSuccess => &self.after_start_success,
            HookName::AfterStartFailure => &self.after_start_failure,
            HookName::BeforeStop => &self.before_stop,
            HookName::AfterStopSuccess => &self.after_stop_success,
            HookName::AfterStopTimeout => &self.after_stop_timeout,
            HookName::AfterStopFailure => &self.after_stop_failure,
            HookName::AfterRuntimeExitUnexpected => &self.after_runtime_exit_unexpected,
        }
    }

    pub fn references(&self) -> Vec<(HookName, &str)> {
        let mut references = Vec::new();
        for hook in HookName::all() {
            for action_name in self.actions_for(hook) {
                references.push((hook, action_name.as_str()));
            }
        }
        references
    }
}

impl DefaultsConfig {
    fn is_empty(&self) -> bool {
        self.stop_timeout_secs.is_none() && self.restart.is_none()
    }
}

impl HookName {
    pub fn all() -> [Self; 8] {
        [
            Self::BeforeStart,
            Self::AfterStartSuccess,
            Self::AfterStartFailure,
            Self::BeforeStop,
            Self::AfterStopSuccess,
            Self::AfterStopTimeout,
            Self::AfterStopFailure,
            Self::AfterRuntimeExitUnexpected,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeStart => "before_start",
            Self::AfterStartSuccess => "after_start_success",
            Self::AfterStartFailure => "after_start_failure",
            Self::BeforeStop => "before_stop",
            Self::AfterStopSuccess => "after_stop_success",
            Self::AfterStopTimeout => "after_stop_timeout",
            Self::AfterStopFailure => "after_stop_failure",
            Self::AfterRuntimeExitUnexpected => "after_runtime_exit_unexpected",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "before_start" => Some(Self::BeforeStart),
            "after_start_success" => Some(Self::AfterStartSuccess),
            "after_start_failure" => Some(Self::AfterStartFailure),
            "before_stop" => Some(Self::BeforeStop),
            "after_stop_success" => Some(Self::AfterStopSuccess),
            "after_stop_timeout" => Some(Self::AfterStopTimeout),
            "after_stop_failure" => Some(Self::AfterStopFailure),
            "after_runtime_exit_unexpected" => Some(Self::AfterRuntimeExitUnexpected),
            _ => None,
        }
    }

    pub fn supports_placeholder(self, placeholder: &str) -> bool {
        match placeholder {
            "project_root" | "config_path" | "service_name" | "action_name" | "hook_name"
            | "service_cwd" | "service_executable" => true,
            "service_pid" => !matches!(self, Self::BeforeStart),
            "stop_reason" => matches!(
                self,
                Self::BeforeStop
                    | Self::AfterStopSuccess
                    | Self::AfterStopTimeout
                    | Self::AfterStopFailure
            ),
            "exit_code" | "exit_status" => matches!(
                self,
                Self::AfterStartFailure
                    | Self::AfterStopSuccess
                    | Self::AfterStopFailure
                    | Self::AfterRuntimeExitUnexpected
            ),
            _ => false,
        }
    }
}

impl ActionRenderContext {
    pub fn render_args(&self, args: &[String]) -> AppResult<Vec<String>> {
        self.prepare(args).map(|prepared| prepared.rendered_args)
    }

    pub fn prepare(&self, args: &[String]) -> AppResult<PreparedActionExecution> {
        let placeholders = referenced_placeholder_names(args)?;
        let mut resolved_params = BTreeMap::new();
        for placeholder in placeholders {
            let value = self.resolve_placeholder(&placeholder).ok_or_else(|| {
                AppError::config_invalid(format!(
                    "placeholder `${{{placeholder}}}` is not available"
                ))
            })?;
            resolved_params.insert(placeholder, value);
        }

        let rendered_args = args
            .iter()
            .map(|arg| {
                render_placeholders(arg, |placeholder| self.resolve_placeholder(placeholder))
            })
            .collect::<AppResult<Vec<_>>>()?;

        Ok(PreparedActionExecution {
            rendered_args,
            resolved_params,
        })
    }

    fn resolve_placeholder(&self, placeholder: &str) -> Option<String> {
        match placeholder {
            "project_root" => Some(self.project_root.display().to_string()),
            "config_path" => Some(self.config_path.display().to_string()),
            "service_name" => Some(self.service_name.clone()),
            "action_name" => Some(self.action_name.clone()),
            "hook_name" => Some(self.hook_name.clone()),
            "service_cwd" => Some(self.service_cwd.display().to_string()),
            "service_executable" => Some(self.service_executable.clone()),
            "service_pid" => self.service_pid.clone(),
            "stop_reason" => self.stop_reason.clone(),
            "exit_code" => self.exit_code.clone(),
            "exit_status" => self.exit_status.clone(),
            _ => None,
        }
    }
}

fn is_valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

fn is_valid_action_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphanumeric() => {}
        _ => return false,
    }

    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn executable_exists(executable: &str, working_dir: &Path) -> bool {
    if executable.contains(std::path::MAIN_SEPARATOR)
        || executable.contains('/')
        || executable.contains('\\')
    {
        let candidate = Path::new(executable);
        if candidate.is_absolute() {
            return candidate.is_file();
        }
        return working_dir.join(candidate).is_file();
    }

    let Some(paths) = env::var_os("PATH") else {
        return false;
    };

    let path_exts = executable_extensions();
    env::split_paths(&paths).any(|dir| {
        if path_exts.is_empty() {
            return dir.join(executable).is_file();
        }

        path_exts.iter().any(|ext| {
            let candidate = if ext.is_empty() {
                dir.join(executable)
            } else {
                dir.join(format!("{executable}{ext}"))
            };
            candidate.is_file()
        })
    })
}

fn resolve_path(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn default_log_append() -> bool {
    true
}

fn default_rotate_log(path: &str) -> LogConfig {
    LogConfig {
        file: PathBuf::from(path),
        append: true,
        max_file_bytes: Some(10 * 1024 * 1024),
        overflow_strategy: Some(LogOverflowStrategy::Rotate),
        rotate_file_count: Some(5),
    }
}

fn validate_log_config(owner_label: &str, log: &LogConfig) -> AppResult<()> {
    if let Some(max_file_bytes) = log.max_file_bytes
        && max_file_bytes == 0
    {
        return Err(AppError::config_invalid(format!(
            "{owner_label} log.max_file_bytes must be greater than 0"
        )));
    }

    match (
        &log.max_file_bytes,
        &log.overflow_strategy,
        &log.rotate_file_count,
    ) {
        (None, None, None) => Ok(()),
        (Some(_), Some(LogOverflowStrategy::Rotate), Some(count)) if *count > 0 => Ok(()),
        (Some(_), Some(LogOverflowStrategy::Archive), None) => Ok(()),
        (Some(_), None, None) => Err(AppError::config_invalid(format!(
            "{owner_label} log.overflow_strategy is required when log.max_file_bytes is set"
        ))),
        (Some(_), Some(LogOverflowStrategy::Rotate), None) => {
            Err(AppError::config_invalid(format!(
                "{owner_label} log.rotate_file_count is required when log.overflow_strategy is `rotate`"
            )))
        }
        (Some(_), Some(LogOverflowStrategy::Rotate), Some(_)) => Err(AppError::config_invalid(
            format!("{owner_label} log.rotate_file_count must be greater than 0"),
        )),
        (Some(_), Some(LogOverflowStrategy::Archive), Some(_)) => {
            Err(AppError::config_invalid(format!(
                "{owner_label} log.rotate_file_count is only valid when log.overflow_strategy is `rotate`"
            )))
        }
        (None, Some(_), _) => Err(AppError::config_invalid(format!(
            "{owner_label} log.max_file_bytes is required when log.overflow_strategy is set"
        ))),
        (None, None, Some(_)) => Err(AppError::config_invalid(format!(
            "{owner_label} log.rotate_file_count requires log.max_file_bytes and log.overflow_strategy"
        ))),
        (Some(_), None, Some(_)) => Err(AppError::config_invalid(format!(
            "{owner_label} log.rotate_file_count requires log.overflow_strategy = `rotate`"
        ))),
    }
}

fn validate_service_watch_config(
    owner_label: &str,
    watch: &ServiceWatchConfig,
    project_root: &Path,
) -> AppResult<()> {
    if watch.paths.is_empty() {
        return Err(AppError::config_invalid(format!(
            "{owner_label} watch.paths must be a non-empty array"
        )));
    }

    if let Some(debounce_ms) = watch.debounce_ms
        && debounce_ms == 0
    {
        return Err(AppError::config_invalid(format!(
            "{owner_label} watch.debounce_ms must be greater than 0"
        )));
    }

    let mut seen = BTreeSet::new();
    for (index, path) in watch.paths.iter().enumerate() {
        if path.as_os_str().is_empty() {
            return Err(AppError::config_invalid(format!(
                "{owner_label} watch.paths[{index}] must not be empty"
            )));
        }

        let resolved = resolve_path(project_root, path);
        if !resolved.exists() {
            return Err(AppError::config_invalid(format!(
                "{owner_label} watch.paths[{index}] resolved watch path does not exist: {}",
                resolved.display()
            )));
        }

        let metadata = fs::symlink_metadata(&resolved).map_err(|error| {
            AppError::config_invalid(format!(
                "{owner_label} watch.paths[{index}] failed to inspect {}: {error}",
                resolved.display()
            ))
        })?;
        let file_type = metadata.file_type();
        if !file_type.is_file() && !file_type.is_dir() && !file_type.is_symlink() {
            return Err(AppError::config_invalid(format!(
                "{owner_label} watch.paths[{index}] must resolve to a file or directory: {}",
                resolved.display()
            )));
        }

        if !seen.insert(resolved.clone()) {
            return Err(AppError::config_invalid(format!(
                "{owner_label} watch.paths contains duplicate resolved path: {}",
                resolved.display()
            )));
        }
    }

    Ok(())
}

fn resolve_log_config(project_root: &Path, log: &LogConfig) -> ResolvedLogConfig {
    ResolvedLogConfig {
        file: resolve_path(project_root, &log.file),
        append: log.append,
        max_file_bytes: log.max_file_bytes,
        overflow_strategy: log.overflow_strategy.clone(),
        rotate_file_count: log.rotate_file_count,
    }
}

fn resolve_watch_config(
    project_root: &Path,
    watch: &ServiceWatchConfig,
) -> ResolvedServiceWatchConfig {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for path in &watch.paths {
        let resolved = resolve_path(project_root, path);
        if seen.insert(resolved.clone()) {
            paths.push(resolved);
        }
    }

    ResolvedServiceWatchConfig {
        paths,
        debounce_ms: watch.debounce_ms.unwrap_or(500),
    }
}

fn placeholder_names(args: &[String]) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for (index, arg) in args.iter().enumerate() {
        for placeholder in scan_placeholders(arg)
            .map_err(|error| format!("contains invalid placeholder at args[{index}]: {error}"))?
        {
            if !is_known_placeholder(&placeholder) {
                return Err(format!(
                    "contains unknown placeholder `${{{placeholder}}}` at args[{index}]"
                ));
            }
            names.insert(placeholder);
        }
    }
    Ok(names)
}

fn validate_placeholders(args: &[String]) -> Result<(), String> {
    placeholder_names(args).map(|_| ())
}

pub fn referenced_placeholder_names(args: &[String]) -> AppResult<BTreeSet<String>> {
    placeholder_names(args).map_err(AppError::config_invalid)
}

fn scan_placeholders(input: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0;
    let mut placeholders = Vec::new();

    while index < chars.len() {
        if chars[index] == '$' && chars.get(index + 1) == Some(&'{') {
            let mut end = index + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }

            if end >= chars.len() {
                return Err(format!("unterminated placeholder in `{input}`"));
            }

            let name: String = chars[index + 2..end].iter().collect();
            if name.is_empty() {
                return Err(format!("empty placeholder in `{input}`"));
            }
            if !name
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
            {
                return Err(format!("invalid placeholder `${{{name}}}` in `{input}`"));
            }

            placeholders.push(name);
            index = end + 1;
            continue;
        }

        index += 1;
    }

    Ok(placeholders)
}

pub fn render_placeholders<F>(input: &str, mut resolver: F) -> AppResult<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0;
    let mut rendered = String::new();

    while index < chars.len() {
        if chars[index] == '$' && chars.get(index + 1) == Some(&'{') {
            let mut end = index + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }

            if end >= chars.len() {
                return Err(AppError::config_invalid(format!(
                    "unterminated placeholder in `{input}`"
                )));
            }

            let name: String = chars[index + 2..end].iter().collect();
            let value = resolver(&name).ok_or_else(|| {
                AppError::config_invalid(format!("placeholder `${{{name}}}` is not available"))
            })?;
            rendered.push_str(&value);
            index = end + 1;
            continue;
        }

        rendered.push(chars[index]);
        index += 1;
    }

    Ok(rendered)
}

fn is_known_placeholder(name: &str) -> bool {
    matches!(
        name,
        "project_root"
            | "config_path"
            | "service_name"
            | "action_name"
            | "hook_name"
            | "service_cwd"
            | "service_executable"
            | "service_pid"
            | "stop_reason"
            | "exit_code"
            | "exit_status"
    )
}

pub fn is_known_placeholder_name(name: &str) -> bool {
    is_known_placeholder(name)
}

fn join_config_errors(errors: Vec<String>) -> String {
    if errors.len() == 1 {
        return errors.into_iter().next().unwrap_or_default();
    }

    let mut message = format!("found {} configuration errors:", errors.len());
    for (index, error) in errors.into_iter().enumerate() {
        message.push_str(&format!("\n{}. {error}", index + 1));
    }
    message
}

fn executable_extensions() -> Vec<String> {
    #[cfg(windows)]
    {
        env::var_os("PATHEXT")
            .unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"))
            .to_string_lossy()
            .split(';')
            .map(|value| value.trim().to_ascii_lowercase())
            .collect()
    }

    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{ActionRenderContext, HookName, ProjectConfig};

    #[test]
    fn rejects_dependency_cycles() {
        let raw = r#"
services:
  api:
    executable: "sleep"
    depends_on: ["worker"]
  worker:
    executable: "sleep"
    depends_on: ["api"]
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate("onekey-tasks.yaml".as_ref()).unwrap_err();
        assert!(error.to_string().contains("dependency cycle"));
    }

    #[test]
    fn rejects_rotate_without_rotate_file_count() {
        let raw = r#"
services:
  app:
    executable: "sleep"
    log:
      file: "./logs/app.log"
      max_file_bytes: 1024
      overflow_strategy: "rotate"
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate("onekey-tasks.yaml".as_ref()).unwrap_err();
        assert!(error.to_string().contains("rotate_file_count is required"));
    }

    #[test]
    fn rejects_rotate_file_count_for_archive() {
        let raw = r#"
services:
  app:
    executable: "sleep"
    log:
      file: "./logs/app.log"
      max_file_bytes: 1024
      overflow_strategy: "archive"
      rotate_file_count: 3
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate("onekey-tasks.yaml".as_ref()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only valid when log.overflow_strategy is `rotate`")
        );
    }

    #[test]
    fn rejects_top_level_log_conflict_with_service_log() {
        let raw = r#"
log:
  file: "./logs/shared.log"
services:
  app:
    executable: "sleep"
    log:
      file: "./logs/shared.log"
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate("onekey-tasks.yaml".as_ref()).unwrap_err();
        assert!(error.to_string().contains("top-level log.file conflicts"));
    }

    #[test]
    fn resolves_top_level_log_relative_to_config_root() {
        let raw = r#"
log:
  file: "./logs/instance.log"
services:
  app:
    executable: "sleep"
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let resolved = config
            .resolve_project_log("/tmp/project".as_ref())
            .expect("top-level log should resolve");
        assert_eq!(
            resolved.file,
            std::path::PathBuf::from("/tmp/project/./logs/instance.log")
        );
    }

    #[test]
    fn rejects_unknown_placeholder_in_action() {
        let raw = r#"
actions:
  prepare:
    executable: "echo"
    args: ["${service_naem}"]
services:
  api:
    executable: "sleep"
    hooks:
      before_start: ["prepare"]
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate("onekey-tasks.yaml".as_ref()).unwrap_err();
        assert!(error.to_string().contains("unknown placeholder"));
    }

    #[test]
    fn rejects_empty_watch_paths() {
        let raw = r#"
services:
  api:
    executable: "sleep"
    watch:
      paths: []
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate("onekey-tasks.yaml".as_ref()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("watch.paths must be a non-empty array")
        );
    }

    #[test]
    fn rejects_watch_paths_that_do_not_exist() {
        let dir = temp_dir("watch-missing");
        let path = dir.join("onekey-tasks.yaml");
        let raw = r#"
services:
  api:
    executable: "sleep"
    watch:
      paths: ["./missing.txt"]
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("resolved watch path does not exist")
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_zero_watch_debounce() {
        let dir = temp_dir("watch-debounce-zero");
        let watched = dir.join("src");
        let path = dir.join("onekey-tasks.yaml");
        fs::create_dir_all(&watched).unwrap();

        let raw = r#"
services:
  api:
    executable: "sleep"
    watch:
      paths: ["./src"]
      debounce_ms: 0
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("watch.debounce_ms must be greater than 0")
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolves_watch_paths_relative_to_config_root() {
        let dir = temp_dir("watch-resolve");
        let src_dir = dir.join("src");
        let watched_file = dir.join("Cargo.toml");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(&watched_file, "[package]\nname = \"demo\"\n").unwrap();

        let raw = r#"
services:
  api:
    executable: "sleep"
    watch:
      paths: ["./src", "./Cargo.toml"]
      debounce_ms: 750
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        config.validate(&dir.join("onekey-tasks.yaml")).unwrap();
        let resolved = config.resolve_service("api", &dir).unwrap();

        let watch = resolved.watch.expect("watch should resolve");
        assert_eq!(watch.debounce_ms, 750);
        assert_eq!(watch.paths.len(), 2);
        assert!(watch.paths.contains(&src_dir));
        assert!(watch.paths.contains(&watched_file));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn accepts_service_names_with_underscore() {
        let raw = r#"
services:
  api_server:
    executable: "sleep"
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        config.validate("onekey-tasks.yaml".as_ref()).unwrap();
    }

    #[test]
    fn rejects_hook_placeholder_not_available() {
        let raw = r#"
actions:
  prepare:
    executable: "echo"
    args: ["${service_pid}"]
services:
  api:
    executable: "sleep"
    hooks:
      before_start: ["prepare"]
"#;

        let config: ProjectConfig = serde_yaml::from_str(raw).unwrap();
        let error = config.validate("onekey-tasks.yaml".as_ref()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot use placeholder `${service_pid}`")
        );
    }

    #[test]
    fn renders_action_placeholders() {
        let context = ActionRenderContext {
            project_root: "/tmp/project".into(),
            config_path: "/tmp/project/onekey-tasks.yaml".into(),
            service_name: "api".to_owned(),
            action_name: "prepare".to_owned(),
            hook_name: HookName::BeforeStart.as_str().to_owned(),
            service_cwd: "/tmp/project/backend".into(),
            service_executable: "cargo".to_owned(),
            service_pid: None,
            stop_reason: None,
            exit_code: None,
            exit_status: None,
        };

        let rendered = context
            .render_args(&[
                "${service_name}".to_owned(),
                "${action_name}".to_owned(),
                "${service_cwd}".to_owned(),
            ])
            .unwrap();

        assert_eq!(rendered[0], "api");
        assert_eq!(rendered[1], "prepare");
        assert!(rendered[2].contains("/tmp/project/backend"));
    }

    #[test]
    fn prepares_action_params_for_only_used_placeholders() {
        let context = ActionRenderContext {
            project_root: "/tmp/project".into(),
            config_path: "/tmp/project/onekey-tasks.yaml".into(),
            service_name: "api".to_owned(),
            action_name: "notify".to_owned(),
            hook_name: HookName::BeforeStart.as_str().to_owned(),
            service_cwd: "/tmp/project/backend".into(),
            service_executable: "cargo".to_owned(),
            service_pid: Some(String::new()),
            stop_reason: Some("manual".to_owned()),
            exit_code: Some(String::new()),
            exit_status: Some("manual".to_owned()),
        };

        let prepared = context
            .prepare(&[
                "--service".to_owned(),
                "${service_name}".to_owned(),
                "--hook".to_owned(),
                "${hook_name}".to_owned(),
            ])
            .unwrap();

        assert_eq!(prepared.rendered_args[1], "api");
        assert_eq!(prepared.rendered_args[3], "before_start");
        assert_eq!(prepared.resolved_params.len(), 2);
        assert_eq!(prepared.resolved_params["service_name"], "api");
        assert_eq!(prepared.resolved_params["hook_name"], "before_start");
    }

    #[test]
    fn project_config_can_serialize_to_yaml() {
        let raw = ProjectConfig::preset_minimal().to_yaml_string().unwrap();

        assert!(raw.contains("services:"));
        assert!(raw.contains("onekey-run.log"));
    }

    #[test]
    fn empty_collections_are_omitted_when_serializing() {
        let raw = ProjectConfig::preset_minimal().to_yaml_string().unwrap();

        assert!(!raw.contains("actions:"));
        assert!(!raw.contains("hooks:"));
        assert!(!raw.contains("env: {}"));
    }

    #[test]
    fn preset_minimal_round_trips_through_yaml() {
        let dir = temp_dir("preset-minimal");
        let path = dir.join("onekey-tasks.yaml");

        let config = ProjectConfig::preset_minimal();
        config.validate(&path).unwrap();
        fs::write(&path, config.to_yaml_string().unwrap()).unwrap();

        let loaded = ProjectConfig::load(&path).unwrap();
        assert_eq!(loaded.services.len(), 2);
        assert!(loaded.log.is_some());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn preset_full_round_trips_through_yaml() {
        let dir = temp_dir("preset-full");
        let path = dir.join("onekey-tasks.yaml");

        let config = ProjectConfig::preset_full();
        config.validate(&path).unwrap();
        fs::write(&path, config.to_yaml_string().unwrap()).unwrap();

        let loaded = ProjectConfig::load(&path).unwrap();
        assert_eq!(loaded.services.len(), 2);
        assert_eq!(loaded.actions.len(), 4);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn preset_minimal_windows_uses_cmd_timeout() {
        let config = ProjectConfig::preset_minimal_windows();

        assert_eq!(config.services["app"].executable, "cmd");
        assert_eq!(config.services["worker"].executable, "cmd");
        assert!(
            config.services["app"]
                .args
                .join(" ")
                .contains("timeout /T 30")
        );
    }

    #[test]
    fn preset_full_windows_uses_cmd_for_services_and_actions() {
        let config = ProjectConfig::preset_full_windows();

        assert_eq!(config.services["app"].executable, "cmd");
        assert_eq!(config.services["worker"].executable, "cmd");
        assert_eq!(config.actions["prepare-app"].executable, "cmd");
        assert_eq!(config.actions["notify-up"].executable, "cmd");
    }

    #[test]
    fn preset_full_windows_round_trips_through_yaml() {
        let dir = temp_dir("preset-full-windows");
        let path = dir.join("onekey-tasks.yaml");

        let config = ProjectConfig::preset_full_windows();
        config.validate(&path).unwrap();
        fs::write(&path, config.to_yaml_string().unwrap()).unwrap();

        let loaded = ProjectConfig::load(&path).unwrap();
        assert_eq!(loaded.services["app"].executable, "cmd");
        assert_eq!(loaded.actions["prepare-app"].executable, "cmd");

        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onekey-run-rs-config-{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
