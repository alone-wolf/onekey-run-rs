use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
