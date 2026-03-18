mod app;
mod cli;
mod config;
mod error;
mod orchestrator;
mod process;
mod runtime_state;
mod tui;

use clap::Parser;

use crate::app::run;
use crate::cli::Cli;

fn main() {
    let cli = Cli::parse();

    if let Err(error) = run(cli) {
        eprintln!("{error}");
        std::process::exit(error.exit_code().code());
    }
}
