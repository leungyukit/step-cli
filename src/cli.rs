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

    /// Attach an image to the user message (headless mode). May be repeated.
    #[arg(short = 'i', long = "image")]
    pub image: Vec<PathBuf>,

    /// Attach an audio file to be transcribed before sending the prompt (headless mode).
    #[arg(short = 'a', long = "audio")]
    pub audio: Option<PathBuf>,

    /// ASR model override.
    #[arg(long = "asr-model")]
    pub asr_model: Option<String>,

    /// Maximum number of agent rounds.
    #[arg(long = "max-rounds", default_value_t = 30)]
    pub max_rounds: usize,

    /// Context compression threshold as a fraction of the model context window (0.1-1.0).
    #[arg(long = "context-threshold")]
    pub context_threshold: Option<f32>,

    /// Web search provider (e.g., serper, tavily).
    #[arg(long = "search-provider")]
    pub search_provider: Option<String>,

    /// API key for the configured web search provider.
    #[arg(long = "search-api-key")]
    pub search_api_key: Option<String>,

    /// Disable the TUI and use the line-based REPL.
    #[arg(long = "no-tui")]
    pub no_tui: bool,

    /// Run interactive setup wizard.
    #[arg(long = "setup")]
    pub setup: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Check configuration and API connectivity.
    Doctor,
    /// Run interactive setup wizard.
    Setup,
    /// Log in to the StepFun open platform.
    Login,
    /// Log out from the StepFun open platform.
    Logout,
    /// Transcribe an audio file using StepFun ASR.
    Transcribe {
        /// Path to the audio file.
        audio: PathBuf,
        /// ASR model override.
        #[arg(long = "asr-model")]
        asr_model: Option<String>,
    },
}
