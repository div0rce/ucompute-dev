//! `common` CLI. STUB — implementation pending.
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "common", version, about = "Common — run AI models on the best available compute")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show detected hardware and usable backends
    Hardware { #[arg(long)] json: bool },
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Hardware { json } => {
            let _ = json;
            eprintln!("not implemented");
            std::process::exit(2);
        }
    }
}
