mod cli;
mod orchestrator;
mod watch;
mod work;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    if let Err(e) = cli::run(cli) {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
