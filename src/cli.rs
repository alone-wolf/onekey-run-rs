use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "onekey-run",
    version,
    about = "Lightweight process orchestration for local projects",
    long_about = "onekey-run reads a onekey-tasks.yaml file and orchestrates a set of local service processes."
)]
pub struct Cli {
    #[arg(
        short = 'c',
        long = "config",
        global = true,
        default_value = "onekey-tasks.yaml",
        help = "Path to the configuration file",
        long_help = "Path to the configuration file. Relative paths are resolved from the current working directory."
    )]
    pub config: PathBuf,
    #[arg(long, global = true, help = "Enable more verbose internal output")]
    pub verbose: bool,
    #[arg(long, global = true, help = "Reduce non-essential command output")]
    pub quiet: bool,
    #[arg(long, global = true, help = "Disable colored terminal output")]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Create a configuration template in the current directory")]
    Init(InitArgs),
    #[command(about = "Validate configuration, dependency graph, and executables")]
    Check,
    #[command(about = "List configured services, actions, and dependency relationships")]
    List(ListArgs),
    #[command(about = "Run a single service or action for debugging")]
    Run(RunArgs),
    #[command(about = "Start services defined in the configuration")]
    Up(UpArgs),
    #[command(about = "Stop services started by a previous up command")]
    Down(DownArgs),
    #[command(about = "Show currently running onekey-run instances")]
    Management(ManagementArgs),
    #[command(name = "__daemon-up", hide = true)]
    DaemonUp(DaemonUpArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long, help = "Generate a template with a fuller set of example fields")]
    pub full: bool,
}

#[derive(Debug, Args)]
pub struct UpArgs {
    #[arg(
        long,
        help = "Open the terminal dashboard instead of plain status output"
    )]
    pub tui: bool,
    #[arg(long, requires = "tui", help = "Keep the TUI open after services exit")]
    pub keep: bool,
    #[arg(
        long,
        requires = "keep",
        help = "Allow post-run management actions after services exit"
    )]
    pub manage: bool,
    #[arg(
        short = 'd',
        long = "daemon",
        conflicts_with = "tui",
        help = "Run in the background and return immediately"
    )]
    pub daemon: bool,
    #[arg(help = "Optional service names to start; dependencies are started automatically")]
    pub services: Vec<String>,
}

#[derive(Debug, Args, Clone)]
pub struct ListArgs {
    #[arg(
        long,
        help = "List both services and actions; equivalent to --services --actions"
    )]
    pub all: bool,
    #[arg(long, help = "List services only")]
    pub services: bool,
    #[arg(long, help = "List actions only")]
    pub actions: bool,
    #[arg(
        long,
        conflicts_with = "dag",
        help = "Show detailed configuration fields"
    )]
    pub detail: bool,
    #[arg(
        long = "DAG",
        alias = "dag",
        conflicts_with_all = ["all", "services", "actions", "detail"],
        help = "Show service dependencies and hook-to-action references as a DAG-style edge list"
    )]
    pub dag: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct KeyValueArg {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Args, Clone)]
#[command(
    group(
        ArgGroup::new("run_target")
            .args(["service", "action"])
            .required(true)
            .multiple(false)
    ),
    group(
        ArgGroup::new("hook_selection")
            .args(["with_all_hooks", "without_hooks", "hook"])
            .multiple(false)
    )
)]
pub struct RunArgs {
    #[arg(long, group = "run_target", help = "Run a single configured service")]
    pub service: Option<String>,
    #[arg(long, group = "run_target", help = "Run a single configured action")]
    pub action: Option<String>,
    #[arg(
        long,
        requires = "service",
        group = "hook_selection",
        help = "Run all hooks that naturally occur during the single-service lifecycle"
    )]
    pub with_all_hooks: bool,
    #[arg(
        long,
        requires = "service",
        group = "hook_selection",
        help = "Skip all hooks while running the single service"
    )]
    pub without_hooks: bool,
    #[arg(
        long = "hook",
        requires = "service",
        group = "hook_selection",
        help = "Run only the selected hook name; may be passed multiple times"
    )]
    pub hook: Vec<String>,
    #[arg(
        long = "arg",
        requires = "action",
        value_parser = parse_key_value_arg,
        help = "Provide or override a standalone action context value as key=value"
    )]
    pub args: Vec<KeyValueArg>,
}

#[derive(Debug, Args)]
pub struct DownArgs {
    #[arg(
        long,
        help = "Force-stop services without waiting for graceful shutdown"
    )]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ManagementArgs {
    #[arg(long, help = "Continuously refresh the instance list until Ctrl-C")]
    pub watch: bool,
    #[arg(
        long,
        conflicts_with = "watch",
        help = "Output the current instance snapshot as JSON"
    )]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DaemonUpArgs {
    #[arg(help = "Internal service target list used by background supervisor mode")]
    pub services: Vec<String>,
}

fn parse_key_value_arg(input: &str) -> Result<KeyValueArg, String> {
    let Some((key, value)) = input.split_once('=') else {
        return Err(format!(
            "invalid `--arg` value `{input}`; expected key=value"
        ));
    };

    if key.is_empty() {
        return Err(format!(
            "invalid `--arg` value `{input}`; key must not be empty"
        ));
    }

    Ok(KeyValueArg {
        key: key.to_owned(),
        value: value.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command};

    #[test]
    fn list_defaults_to_plain_listing() {
        let cli = Cli::try_parse_from(["onekey-run", "list"]).unwrap();
        let Command::List(args) = cli.command else {
            panic!("expected list command");
        };

        assert!(!args.all);
        assert!(!args.services);
        assert!(!args.actions);
        assert!(!args.detail);
        assert!(!args.dag);
    }

    #[test]
    fn list_accepts_detail_and_scope_flags() {
        let cli = Cli::try_parse_from(["onekey-run", "list", "--detail", "--services"]).unwrap();
        let Command::List(args) = cli.command else {
            panic!("expected list command");
        };

        assert!(args.detail);
        assert!(args.services);
        assert!(!args.actions);
        assert!(!args.dag);
    }

    #[test]
    fn list_accepts_uppercase_dag_flag() {
        let cli = Cli::try_parse_from(["onekey-run", "list", "--DAG"]).unwrap();
        let Command::List(args) = cli.command else {
            panic!("expected list command");
        };

        assert!(args.dag);
    }

    #[test]
    fn list_rejects_conflicting_detail_and_dag_flags() {
        let error = Cli::try_parse_from(["onekey-run", "list", "--detail", "--DAG"]).unwrap_err();
        let rendered = error.to_string();

        assert!(rendered.contains("--detail"));
        assert!(rendered.contains("--DAG"));
    }

    #[test]
    fn run_rejects_service_and_action_together() {
        let error = Cli::try_parse_from([
            "onekey-run",
            "run",
            "--service",
            "api",
            "--action",
            "prepare",
        ])
        .unwrap_err();

        assert!(error.to_string().contains("--action"));
    }

    #[test]
    fn run_rejects_conflicting_hook_selection_flags() {
        let error = Cli::try_parse_from([
            "onekey-run",
            "run",
            "--service",
            "api",
            "--with-all-hooks",
            "--without-hooks",
        ])
        .unwrap_err();

        let rendered = error.to_string();
        assert!(rendered.contains("--with-all-hooks"));
        assert!(rendered.contains("--without-hooks"));
    }

    #[test]
    fn run_accepts_multiple_hooks() {
        let cli = Cli::try_parse_from([
            "onekey-run",
            "run",
            "--service",
            "api",
            "--hook",
            "before_start",
            "--hook",
            "after_start_success",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("expected run command");
        };

        assert_eq!(args.service.as_deref(), Some("api"));
        assert_eq!(
            args.hook,
            vec!["before_start".to_owned(), "after_start_success".to_owned()]
        );
    }

    #[test]
    fn run_accepts_multiple_action_args() {
        let cli = Cli::try_parse_from([
            "onekey-run",
            "run",
            "--action",
            "notify",
            "--arg",
            "service_name=api",
            "--arg",
            "hook_name=manual",
        ])
        .unwrap();
        let Command::Run(args) = cli.command else {
            panic!("expected run command");
        };

        assert_eq!(args.action.as_deref(), Some("notify"));
        assert_eq!(args.args.len(), 2);
        assert_eq!(args.args[0].key, "service_name");
        assert_eq!(args.args[0].value, "api");
        assert_eq!(args.args[1].key, "hook_name");
        assert_eq!(args.args[1].value, "manual");
    }

    #[test]
    fn up_rejects_keep_without_tui() {
        let error = Cli::try_parse_from(["onekey-run", "up", "--keep"]).unwrap_err();
        let rendered = error.to_string();

        assert!(rendered.contains("--keep"));
        assert!(rendered.contains("--tui"));
    }

    #[test]
    fn up_rejects_manage_without_keep() {
        let error = Cli::try_parse_from(["onekey-run", "up", "--manage"]).unwrap_err();
        let rendered = error.to_string();

        assert!(rendered.contains("--manage"));
        assert!(rendered.contains("--keep"));
    }

    #[test]
    fn up_rejects_manage_without_keep_when_tui_enabled() {
        let error = Cli::try_parse_from(["onekey-run", "up", "--tui", "--manage"]).unwrap_err();
        let rendered = error.to_string();

        assert!(rendered.contains("--manage"));
        assert!(rendered.contains("--keep"));
    }

    #[test]
    fn up_accepts_keep_with_tui() {
        let cli = Cli::try_parse_from(["onekey-run", "up", "--tui", "--keep"]).unwrap();
        let Command::Up(args) = cli.command else {
            panic!("expected up command");
        };

        assert!(args.tui);
        assert!(args.keep);
        assert!(!args.manage);
    }

    #[test]
    fn up_accepts_manage_with_keep() {
        let cli = Cli::try_parse_from(["onekey-run", "up", "--tui", "--keep", "--manage"]).unwrap();
        let Command::Up(args) = cli.command else {
            panic!("expected up command");
        };

        assert!(args.tui);
        assert!(args.keep);
        assert!(args.manage);
    }
}
