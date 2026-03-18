use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "onekey-run",
    version,
    about = "Lightweight process orchestration for local projects"
)]
pub struct Cli {
    #[arg(
        short = 'c',
        long = "config",
        global = true,
        default_value = "onekey-tasks.yaml"
    )]
    pub config: PathBuf,
    #[arg(long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub quiet: bool,
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Init(InitArgs),
    Check,
    Up(UpArgs),
    Down(DownArgs),
    Management(ManagementArgs),
    #[command(name = "__daemon-up", hide = true)]
    DaemonUp(DaemonUpArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub full: bool,
}

#[derive(Debug, Args)]
pub struct UpArgs {
    #[arg(long)]
    pub tui: bool,
    #[arg(short = 'd', long = "daemon", conflicts_with = "tui")]
    pub daemon: bool,
    pub services: Vec<String>,
}

#[derive(Debug, Args)]
pub struct DownArgs {
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ManagementArgs {
    #[arg(long)]
    pub watch: bool,
    #[arg(long, conflicts_with = "watch")]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct DaemonUpArgs {
    pub services: Vec<String>,
}
