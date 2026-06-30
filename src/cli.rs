use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "step")]
#[command(about = "A Rust CLI coding agent powered by StepFun models")]
#[command(version)]
pub struct Cli {
    /// Single prompt mode: run one request and exit.
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,

    /// Workspace directory (defaults to current directory).
    #[arg(short = 'w', long = "workspace")]
    pub workspace: Option<PathBuf>,

    /// YOLO mode: auto-approve all tool calls.
    #[arg(long = "yolo")]
    pub yolo: bool,

    /// Trust files outside the workspace.
    #[arg(long = "trust")]
    pub trust: bool,

    /// Override the StepFun API base URL.
    #[arg(long = "base-url")]
    pub base_url: Option<String>,

    /// Override the model name.
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// Maximum number of agent rounds.
    #[arg(long = "max-rounds", default_value_t = 30)]
    pub max_rounds: usize,

    /// Disable the TUI and use the line-based REPL.
    #[arg(long = "no-tui")]
    pub no_tui: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Check configuration and API connectivity.
    Doctor,
}
