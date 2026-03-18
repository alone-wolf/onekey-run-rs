use std::env;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::{Cli, Command, DaemonUpArgs, ManagementArgs};
use crate::config::ProjectConfig;
use crate::error::{AppError, AppResult};
use crate::orchestrator;
use crate::runtime_state;

pub fn run(cli: Cli) -> AppResult<()> {
    match cli.command {
        Command::Init(args) => orchestrator::run_init(&cli.config, args.full),
        Command::Check => {
            let config = ProjectConfig::load(&cli.config)?;
            let plan = orchestrator::build_run_plan(&config, &cli.config, &[])?;
            orchestrator::run_check(&plan, &config)
        }
        Command::Management(ManagementArgs { watch, json }) => {
            orchestrator::run_management(watch, json)
        }
        Command::Up(args) => {
            let config = ProjectConfig::load(&cli.config)?;
            let plan = orchestrator::build_run_plan(&config, &cli.config, &args.services)?;
            if args.daemon {
                return spawn_daemon_process(&cli.config, &args.services, &plan);
            }
            orchestrator::run_up(
                plan,
                orchestrator::RunOptions {
                    tui: args.tui,
                    daemonized: false,
                },
            )
        }
        Command::Down(args) => {
            let project_root = resolve_project_root_from_config(&cli.config)?;
            orchestrator::run_down(&project_root, args.force)
        }
        Command::DaemonUp(DaemonUpArgs { services }) => {
            let config = ProjectConfig::load(&cli.config)?;
            let plan = orchestrator::build_run_plan(&config, &cli.config, &services)?;
            orchestrator::run_up(
                plan,
                orchestrator::RunOptions {
                    tui: false,
                    daemonized: true,
                },
            )
        }
    }
}

fn resolve_project_root_from_config(config_path: &Path) -> AppResult<PathBuf> {
    let current_dir = env::current_dir().map_err(|error| {
        crate::error::AppError::runtime_failed(format!(
            "failed to resolve current directory: {error}"
        ))
    })?;

    let absolute_config_path = if config_path.is_absolute() {
        config_path.to_path_buf()
    } else {
        current_dir.join(config_path)
    };

    let project_root = absolute_config_path.parent().ok_or_else(|| {
        crate::error::AppError::runtime_failed(format!(
            "failed to resolve project root from config path {}",
            config_path.display()
        ))
    })?;

    if project_root.exists() {
        project_root.canonicalize().map_err(|error| {
            crate::error::AppError::runtime_failed(format!(
                "failed to canonicalize project root {}: {error}",
                project_root.display()
            ))
        })
    } else {
        Ok(project_root.to_path_buf())
    }
}

fn spawn_daemon_process(
    config_path: &Path,
    services: &[String],
    plan: &orchestrator::RunPlan,
) -> AppResult<()> {
    let current_exe = env::current_exe().map_err(|error| {
        AppError::startup_failed(format!("failed to resolve current executable: {error}"))
    })?;

    let mut command = ProcessCommand::new(current_exe);
    command
        .arg("--config")
        .arg(config_path)
        .arg("__daemon-up")
        .args(services)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    configure_daemon_command(&mut command);

    let mut child = command.spawn().map_err(|error| {
        AppError::startup_failed(format!("failed to spawn background supervisor: {error}"))
    })?;

    wait_for_daemon_ready(&mut child, &plan.project_root, plan.services.len())?;

    println!(
        "started {} service(s) in background from {}",
        plan.services.len(),
        plan.config_path.display()
    );
    println!(
        "use `onekey-run down -c {}` to stop them",
        plan.config_path.display()
    );
    Ok(())
}

fn wait_for_daemon_ready(
    child: &mut Child,
    project_root: &Path,
    expected_services: usize,
) -> AppResult<()> {
    let runtime_dir = project_root.join(runtime_state::RUNTIME_DIR);
    let lock_path = runtime_dir.join(runtime_state::LOCK_FILE);
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            AppError::startup_failed(format!(
                "failed to inspect background supervisor status: {error}"
            ))
        })? {
            return Err(AppError::startup_failed(format!(
                "background supervisor exited before startup completed with status {status}"
            )));
        }

        if lock_path.exists()
            && let Ok(state) = runtime_state::load_state(project_root)
            && state.services.len() == expected_services
        {
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(AppError::startup_failed(format!(
                "timed out waiting for background supervisor to finish startup in {}",
                project_root.display()
            )));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(unix)]
fn configure_daemon_command(command: &mut ProcessCommand) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(windows)]
fn configure_daemon_command(command: &mut ProcessCommand) {
    use std::os::windows::process::CommandExt;

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::cli::{Cli, Command, DownArgs};
    use crate::runtime_state::{self, RuntimeState};

    use super::{resolve_project_root_from_config, run};

    fn current_dir_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn down_uses_config_parent_when_current_dir_differs() {
        let _guard = current_dir_lock().lock().unwrap();
        let dir = temp_dir("down-config-root");
        let caller_dir = dir.join("caller");
        let project_root = dir.join("project");
        let config_path = project_root.join("onekey-tasks.yaml");

        fs::create_dir_all(&caller_dir).unwrap();
        fs::create_dir_all(&project_root).unwrap();
        fs::write(&config_path, "services: {}\n").unwrap();
        fs::create_dir_all(project_root.join(runtime_state::RUNTIME_DIR)).unwrap();

        let canonical_project_root = project_root.canonicalize().unwrap();
        let canonical_config_path = config_path.canonicalize().unwrap();

        let state = RuntimeState::new(canonical_project_root.clone(), canonical_config_path);
        runtime_state::write_state(&canonical_project_root, &state).unwrap();

        let original_dir = env::current_dir().unwrap();
        env::set_current_dir(&caller_dir).unwrap();

        let result = run(Cli {
            config: PathBuf::from("../project/onekey-tasks.yaml"),
            verbose: false,
            quiet: false,
            no_color: false,
            command: Command::Down(DownArgs { force: false }),
        });

        env::set_current_dir(&original_dir).unwrap();

        assert!(result.is_ok(), "{result:?}");
        assert!(!runtime_state::state_path(&canonical_project_root).exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn resolve_project_root_from_relative_config_path() {
        let _guard = current_dir_lock().lock().unwrap();
        let dir = temp_dir("resolve-config-root");
        let caller_dir = dir.join("caller");
        let project_root = dir.join("project");
        let config_path = project_root.join("onekey-tasks.yaml");

        fs::create_dir_all(&caller_dir).unwrap();
        fs::create_dir_all(&project_root).unwrap();
        fs::write(&config_path, "services: {}\n").unwrap();

        let original_dir = env::current_dir().unwrap();
        env::set_current_dir(&caller_dir).unwrap();

        let resolved = resolve_project_root_from_config(Path::new("../project/onekey-tasks.yaml"));

        env::set_current_dir(&original_dir).unwrap();

        assert_eq!(resolved.unwrap(), project_root.canonicalize().unwrap());

        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("onekey-run-rs-app-{prefix}-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
