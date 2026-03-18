use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, AppResult};

#[cfg(windows)]
use std::ffi::OsString;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    pub services: BTreeMap<String, ServiceConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsConfig {
    #[serde(default)]
    pub stop_timeout_secs: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub restart: Option<RestartPolicy>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    No,
    OnFailure,
    Always,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub restart: Option<RestartPolicy>,
    #[serde(default)]
    pub stop_signal: Option<String>,
    #[serde(default)]
    pub stop_timeout_secs: Option<u64>,
    #[serde(default)]
    pub autostart: Option<bool>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub log: Option<LogConfig>,
}

#[derive(Debug, Clone)]
pub struct ResolvedServiceConfig {
    pub name: String,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub stop_signal: Option<String>,
    pub stop_timeout_secs: u64,
    pub log: Option<ResolvedLogConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    pub file: PathBuf,
    #[serde(default = "default_log_append")]
    pub append: bool,
    #[serde(default)]
    pub max_file_bytes: Option<u64>,
    #[serde(default)]
    pub overflow_strategy: Option<LogOverflowStrategy>,
    #[serde(default)]
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

#[derive(Debug, Clone, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LogOverflowStrategy {
    Rotate,
    Archive,
}

impl ProjectConfig {
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
        if self.services.is_empty() {
            return Err(AppError::config_invalid(format!(
                "configuration at {} must define at least one service",
                path.display()
            )));
        }

        for (name, service) in &self.services {
            if !is_valid_service_name(name) {
                return Err(AppError::config_invalid(format!(
                    "service name `{name}` is invalid; use lowercase letters, digits, or `-`"
                )));
            }

            if service.executable.trim().is_empty() {
                return Err(AppError::config_invalid(format!(
                    "service `{name}` must define a non-empty executable"
                )));
            }

            if let Some(log) = &service.log
                && log.file.as_os_str().is_empty()
            {
                return Err(AppError::config_invalid(format!(
                    "service `{name}` log.file must not be empty"
                )));
            }
            if let Some(log) = &service.log {
                validate_log_config(name, log)?;
            }

            for dependency in &service.depends_on {
                if !self.services.contains_key(dependency) {
                    return Err(AppError::config_invalid(format!(
                        "service `{name}` depends on unknown service `{dependency}`"
                    )));
                }
            }
        }

        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for name in self.services.keys() {
            self.validate_acyclic(name, &mut visiting, &mut visited)?;
        }

        Ok(())
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
            stop_signal: service.stop_signal.clone(),
            stop_timeout_secs: service
                .stop_timeout_secs
                .or(self.defaults.stop_timeout_secs)
                .unwrap_or(10),
            log: service.log.as_ref().map(|log| ResolvedLogConfig {
                file: resolve_path(project_root, &log.file),
                append: log.append,
                max_file_bytes: log.max_file_bytes,
                overflow_strategy: log.overflow_strategy.clone(),
                rotate_file_count: log.rotate_file_count,
            }),
        })
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
}

impl ServiceConfig {
    fn is_disabled(&self) -> bool {
        self.disabled.unwrap_or(false)
    }
}

fn is_valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
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

fn validate_log_config(service_name: &str, log: &LogConfig) -> AppResult<()> {
    if let Some(max_file_bytes) = log.max_file_bytes
        && max_file_bytes == 0
    {
        return Err(AppError::config_invalid(format!(
            "service `{service_name}` log.max_file_bytes must be greater than 0"
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
            "service `{service_name}` log.overflow_strategy is required when log.max_file_bytes is set"
        ))),
        (Some(_), Some(LogOverflowStrategy::Rotate), None) => {
            Err(AppError::config_invalid(format!(
                "service `{service_name}` log.rotate_file_count is required when log.overflow_strategy is `rotate`"
            )))
        }
        (Some(_), Some(LogOverflowStrategy::Rotate), Some(_)) => Err(AppError::config_invalid(
            format!("service `{service_name}` log.rotate_file_count must be greater than 0"),
        )),
        (Some(_), Some(LogOverflowStrategy::Archive), Some(_)) => {
            Err(AppError::config_invalid(format!(
                "service `{service_name}` log.rotate_file_count is only valid when log.overflow_strategy is `rotate`"
            )))
        }
        (None, Some(_), _) => Err(AppError::config_invalid(format!(
            "service `{service_name}` log.max_file_bytes is required when log.overflow_strategy is set"
        ))),
        (None, None, Some(_)) => Err(AppError::config_invalid(format!(
            "service `{service_name}` log.rotate_file_count requires log.max_file_bytes and log.overflow_strategy"
        ))),
        (Some(_), None, Some(_)) => Err(AppError::config_invalid(format!(
            "service `{service_name}` log.rotate_file_count requires log.overflow_strategy = `rotate`"
        ))),
    }
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
    use super::ProjectConfig;

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
}
