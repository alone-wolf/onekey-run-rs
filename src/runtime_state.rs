use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

pub const RUNTIME_DIR: &str = ".onekey-run";
pub const STATE_FILE: &str = "state.json";
pub const LOCK_FILE: &str = "lock.json";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const REGISTRY_DIR: &str = "onekey-run";
pub const REGISTRY_FILE: &str = "registry.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeState {
    pub instance_id: String,
    pub project_root: PathBuf,
    pub config_path: PathBuf,
    #[serde(default)]
    pub instance_log_file: Option<PathBuf>,
    pub tool_pid: u32,
    pub started_at_unix_secs: u64,
    pub services: Vec<ServiceRuntimeState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub timestamp_unix_secs: u64,
    pub event_type: String,
    pub service_name: Option<String>,
    pub hook_name: Option<String>,
    pub action_name: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRuntimeState {
    pub service_name: String,
    pub pid: u32,
    pub cwd: PathBuf,
    pub executable: String,
    pub args: Vec<String>,
    pub log_file: Option<PathBuf>,
    pub stop_signal: Option<String>,
    pub stop_timeout_secs: u64,
    pub platform: PlatformRuntimeState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlatformRuntimeState {
    pub process_group_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub project_root: PathBuf,
    pub config_path: PathBuf,
    #[serde(default)]
    pub instance_log_file: Option<PathBuf>,
    pub tool_pid: u32,
    pub started_at_unix_secs: u64,
    pub service_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LockRecord {
    instance_id: String,
    tool_pid: u32,
    project_root: PathBuf,
}

pub struct RuntimeLock {
    lock_path: PathBuf,
}

impl RuntimeState {
    pub fn new(
        project_root: PathBuf,
        config_path: PathBuf,
        instance_log_file: Option<PathBuf>,
    ) -> Self {
        Self {
            instance_id: generate_instance_id(),
            project_root,
            config_path,
            instance_log_file,
            tool_pid: process::id(),
            started_at_unix_secs: unix_timestamp(),
            services: Vec::new(),
        }
    }
}

impl RuntimeLock {
    pub fn acquire(project_root: &Path) -> AppResult<Self> {
        let runtime_dir = project_root.join(RUNTIME_DIR);
        fs::create_dir_all(&runtime_dir).map_err(|error| {
            AppError::startup_failed(format!(
                "failed to create runtime directory {}: {error}",
                runtime_dir.display()
            ))
        })?;
        let lock_path = runtime_dir.join(LOCK_FILE);

        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let record = LockRecord {
                    instance_id: generate_instance_id(),
                    tool_pid: process::id(),
                    project_root: project_root.to_path_buf(),
                };
                serde_json::to_writer_pretty(&mut file, &record).map_err(|error| {
                    AppError::startup_failed(format!(
                        "failed to write runtime lock file {}: {error}",
                        lock_path.display()
                    ))
                })?;
                file.write_all(b"\n").map_err(|error| {
                    AppError::startup_failed(format!(
                        "failed to finalize runtime lock file {}: {error}",
                        lock_path.display()
                    ))
                })?;
                Ok(Self { lock_path })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_lock_record(&lock_path)?;
                if crate::process::is_pid_alive(existing.tool_pid) {
                    Err(AppError::startup_failed(format!(
                        "project is already running under pid {}; remove {} only if you are sure it is stale",
                        existing.tool_pid,
                        lock_path.display()
                    )))
                } else {
                    cleanup_runtime_files(project_root)?;
                    Self::acquire(project_root)
                }
            }
            Err(error) => Err(AppError::startup_failed(format!(
                "failed to create runtime lock file {}: {error}",
                lock_path.display()
            ))),
        }
    }

    pub fn release(self) -> AppResult<()> {
        if self.lock_path.exists() {
            fs::remove_file(&self.lock_path).map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to remove runtime lock {}: {error}",
                    self.lock_path.display()
                ))
            })?;
        }
        Ok(())
    }
}

pub fn load_state(project_root: &Path) -> AppResult<RuntimeState> {
    let state_path = state_path(project_root);
    let raw = fs::read_to_string(&state_path).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to read runtime state {}: {error}",
            state_path.display()
        ))
    })?;
    serde_json::from_str(&raw).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to parse runtime state {}: {error}",
            state_path.display()
        ))
    })
}

pub fn write_state(project_root: &Path, state: &RuntimeState) -> AppResult<()> {
    let state_path = state_path(project_root);
    let raw = serde_json::to_vec_pretty(state).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to serialize runtime state {}: {error}",
            state_path.display()
        ))
    })?;
    fs::write(&state_path, raw).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to write runtime state {}: {error}",
            state_path.display()
        ))
    })
}

pub fn cleanup_runtime_files(project_root: &Path) -> AppResult<()> {
    let runtime_dir = project_root.join(RUNTIME_DIR);
    let state_path = runtime_dir.join(STATE_FILE);
    let lock_path = runtime_dir.join(LOCK_FILE);
    let events_path = runtime_dir.join(EVENTS_FILE);

    if state_path.exists() {
        fs::remove_file(&state_path).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to remove runtime state {}: {error}",
                state_path.display()
            ))
        })?;
    }

    if lock_path.exists() {
        fs::remove_file(&lock_path).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to remove runtime lock {}: {error}",
                lock_path.display()
            ))
        })?;
    }

    if events_path.exists() {
        fs::remove_file(&events_path).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to remove runtime events {}: {error}",
                events_path.display()
            ))
        })?;
    }

    if runtime_dir.exists()
        && runtime_dir
            .read_dir()
            .map_err(|error| {
                AppError::runtime_failed(format!(
                    "failed to inspect runtime directory {}: {error}",
                    runtime_dir.display()
                ))
            })?
            .next()
            .is_none()
    {
        fs::remove_dir(&runtime_dir).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to remove runtime directory {}: {error}",
                runtime_dir.display()
            ))
        })?;
    }

    unregister_instance(project_root)?;

    Ok(())
}

pub fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(RUNTIME_DIR).join(STATE_FILE)
}

pub fn events_path(project_root: &Path) -> PathBuf {
    project_root.join(RUNTIME_DIR).join(EVENTS_FILE)
}

pub fn append_event(project_root: &Path, event: &RuntimeEvent) -> AppResult<()> {
    let path = events_path(project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to create runtime event directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to open runtime events {}: {error}",
                path.display()
            ))
        })?;

    serde_json::to_writer(&mut file, event).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to serialize runtime event for {}: {error}",
            path.display()
        ))
    })?;
    file.write_all(b"\n").map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to append runtime event to {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}

pub fn load_events(project_root: &Path) -> AppResult<Vec<RuntimeEvent>> {
    let path = events_path(project_root);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(&path).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to read runtime events {}: {error}",
            path.display()
        ))
    })?;

    let mut events = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event = serde_json::from_str::<RuntimeEvent>(line).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to parse runtime event {} line {}: {error}",
                path.display(),
                index + 1
            ))
        })?;
        events.push(event);
    }

    Ok(events)
}

pub fn register_instance(state: &RuntimeState) -> AppResult<()> {
    let mut entries = load_registry_entries()?;
    entries.retain(|entry| entry.project_root != state.project_root);
    entries.push(RegistryEntry {
        project_root: state.project_root.clone(),
        config_path: state.config_path.clone(),
        instance_log_file: state.instance_log_file.clone(),
        tool_pid: state.tool_pid,
        started_at_unix_secs: state.started_at_unix_secs,
        service_names: state
            .services
            .iter()
            .map(|service| service.service_name.clone())
            .collect(),
    });
    write_registry_entries(&entries)
}

pub fn unregister_instance(project_root: &Path) -> AppResult<()> {
    let mut entries = load_registry_entries()?;
    let original_len = entries.len();
    entries.retain(|entry| entry.project_root != project_root);
    if entries.len() == original_len {
        return Ok(());
    }
    write_registry_entries(&entries)
}

pub fn list_registry_entries() -> AppResult<Vec<RegistryEntry>> {
    load_registry_entries()
}

fn read_lock_record(path: &Path) -> AppResult<LockRecord> {
    let raw = fs::read_to_string(path).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to read runtime lock {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_str(&raw).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to parse runtime lock {}: {error}",
            path.display()
        ))
    })
}

fn registry_path() -> PathBuf {
    std::env::temp_dir().join(REGISTRY_DIR).join(REGISTRY_FILE)
}

fn load_registry_entries() -> AppResult<Vec<RegistryEntry>> {
    let path = registry_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(&path).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to read instance registry {}: {error}",
            path.display()
        ))
    })?;

    serde_json::from_str(&raw).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to parse instance registry {}: {error}",
            path.display()
        ))
    })
}

fn write_registry_entries(entries: &[RegistryEntry]) -> AppResult<()> {
    let path = registry_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::runtime_failed(format!(
                "failed to create instance registry directory {}: {error}",
                parent.display()
            ))
        })?;
    }

    let raw = serde_json::to_vec_pretty(entries).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to serialize instance registry {}: {error}",
            path.display()
        ))
    })?;

    fs::write(&path, raw).map_err(|error| {
        AppError::runtime_failed(format!(
            "failed to write instance registry {}: {error}",
            path.display()
        ))
    })?;

    Ok(())
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must be after unix epoch")
        .as_secs()
}

fn generate_instance_id() -> String {
    format!("{}-{}", process::id(), unix_timestamp())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        append_event, events_path, list_registry_entries, load_events, register_instance,
        unregister_instance, RuntimeEvent,
    };

    use crate::runtime_state::{RuntimeState, ServiceRuntimeState};

    #[test]
    fn registry_round_trip_register_and_unregister() {
        let dir = temp_dir("registry");
        let project_root = dir.join("project");
        let config_path = project_root.join("onekey-tasks.yaml");
        fs::create_dir_all(&project_root).unwrap();
        let mut state = RuntimeState::new(
            project_root.clone(),
            config_path,
            Some(project_root.join("logs").join("onekey-run.log")),
        );
        state.services.push(ServiceRuntimeState {
            service_name: "app".to_owned(),
            pid: 42,
            cwd: project_root.clone(),
            executable: "sleep".to_owned(),
            args: vec!["30".to_owned()],
            log_file: None,
            stop_signal: None,
            stop_timeout_secs: 10,
            platform: Default::default(),
        });

        let before = list_registry_entries().unwrap();
        register_instance(&state).unwrap();
        let entries = list_registry_entries().unwrap();
        let matching = entries
            .iter()
            .find(|entry| entry.project_root == project_root)
            .unwrap();
        assert_eq!(matching.service_names, vec!["app".to_owned()]);
        assert_eq!(
            matching.instance_log_file,
            Some(project_root.join("logs").join("onekey-run.log"))
        );

        unregister_instance(&project_root).unwrap();
        let after = list_registry_entries().unwrap();
        assert!(after.iter().all(|entry| entry.project_root != project_root));
        assert!(after.len() <= before.len());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn appends_runtime_events_as_jsonl() {
        let dir = temp_dir("events");
        let project_root = dir.join("project");
        fs::create_dir_all(project_root.join(super::RUNTIME_DIR)).unwrap();

        append_event(
            &project_root,
            &RuntimeEvent {
                timestamp_unix_secs: 1,
                event_type: "hook_started".to_owned(),
                service_name: Some("api".to_owned()),
                hook_name: Some("before_start".to_owned()),
                action_name: None,
                detail: "started".to_owned(),
            },
        )
        .unwrap();

        let raw = fs::read_to_string(events_path(&project_root)).unwrap();
        assert!(raw.contains("\"event_type\":\"hook_started\""));
        assert!(raw.contains("\"service_name\":\"api\""));
        let events = load_events(&project_root).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "hook_started");

        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onekey-run-rs-runtime-{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
