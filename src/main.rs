pub mod auth;
pub mod chat;
pub mod cli;
pub mod config;
pub mod runtime;
pub mod setup;
pub mod skills;
pub mod tools;
pub mod ui;

use anyhow::{bail, Context, Result};
use auth::PlatformAuth;
use chat::approver::{AutoApprover, ConsoleApprover};
use chat::asr::{self, AsrClient};
use chat::client::{ChatClient, StreamEvent};
use chat::executor::Executor;
use chat::session::{Message, Session};
use chat::tools::{ToolContext, ToolRegistry};
use chat::vision;
use clap::Parser;
use cli::{Cli, Commands};
use config::Config;
use futures_util::StreamExt;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tools::fs::{
    EditFileTool, GlobFilesTool, GrepFilesTool, ListDirTool, ReadFileTool, WriteFileTool,
};
use tools::mcp::load_mcp_tools;
use tools::shell::ExecShellTool;
use tools::web_search::WebSearchTool;

const SYSTEM_PROMPT: &str = r#"You are step-cli, an AI coding assistant running on the user's local machine.
You have access to the workspace and a set of tools. Use tools only when they are genuinely needed.

## Intent routing — choose the right strategy
For every user message, autonomously pick one or more of the following strategies:

1. Direct answer (no tools)
   Use when the question is about general knowledge, explanations, math, algorithms, coding concepts, or anything you already know confidently. Examples: "Explain Rust lifetimes", "What is a closure?", "How do I reverse a linked list?"

2. Local file lookup
   Use when the question is about the current workspace, project code, configuration, or local files. Tools: read_file, list_dir, glob_files, grep_files. Examples: "What does this function do?", "Find where the API key is loaded", "Show me the project structure".

3. Web search
   Use when the user asks about recent events, latest library versions, external documentation, public APIs, or facts that may have changed after your training data. Tool: web_search. Examples: "What is the latest React version?", "Rust 1.85 release notes", "Serper API pricing".

4. Vision / image analysis
   Use when the user message contains image attachments, or when the user describes or asks about visual content (screenshots, UI, design mockups, diagrams, photos, error dialogs). Directly analyze the attached image and answer based on what you see. If the user mentions an image but did not attach one, suggest using `/image <path>` or `![alt](path)` to attach it. Examples: "Why does this UI show an error?", "Analyze this design mockup", "What is wrong with this code screenshot?"

5. Audio transcription
   Use when the user provides an audio file or asks to transcribe / convert speech to text. The ASR tool will turn the audio into text; you can then answer or act on the transcript. Example: "Summarize this meeting recording" → transcribe the audio, then summarize.

6. Combine strategies
   For complex requests, chain strategies. Example: "How do I use the latest React 19 features in this project?" → web_search for React 19 docs, then read local files, then answer. Another example: analyze a screenshot while reading the actual source file it shows.

Guidelines:
- Prefer direct answers for general knowledge to avoid unnecessary latency.
- Prefer local lookup when the answer clearly depends on the workspace.
- Use web search when freshness or external facts matter.
- When an image is attached, actively look at it and reference what you see.
- When audio is attached or the user asks for transcription, transcribe first and then process the text.
- When combining, explain your plan briefly before acting.
- Prefer reading before writing.
- When editing files, use the exact edit_file tool with old_string/new_string.
- Do not run destructive commands. The user must approve shell commands and file writes unless --yolo is enabled.
"#;

fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadFileTool));
    registry.register(Arc::new(WriteFileTool));
    registry.register(Arc::new(EditFileTool));
    registry.register(Arc::new(ListDirTool));
    registry.register(Arc::new(GlobFilesTool));
    registry.register(Arc::new(GrepFilesTool));
    registry.register(Arc::new(ExecShellTool));
    registry.register(Arc::new(WebSearchTool));
    registry
}

fn print_flush(s: &str) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(s.as_bytes());
    let _ = stdout.flush();
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut config = Config::load()?;
    config.apply_cli(&cli);

    if let Some(Commands::Doctor) = cli.command {
        return run_doctor(&config).await;
    }
    if matches!(cli.command, Some(Commands::Login)) {
        PlatformAuth::login_interactive().await?;
        return Ok(());
    }
    if matches!(cli.command, Some(Commands::Logout)) {
        PlatformAuth::logout()?;
        println!("已退出 StepFun 开放平台登录。");
        return Ok(());
    }
    if let Some(Commands::Transcribe { audio, asr_model }) = cli.command {
        let client = AsrClient::from_config(
            &config.base_url,
            &config.api_key,
            asr_model.as_deref().or(config.asr_model.as_deref()),
        );
        let path = asr::resolve_audio_path(
            &audio.to_string_lossy(),
            config.workspace.as_ref().unwrap(),
            config.trust,
        )?;
        let text = client.transcribe(&path).await?;
        println!("{}", text);
        return Ok(());
    }

    // StepFun platform login check (informational only).
    if !PlatformAuth::load()?.is_authenticated() {
        println!("未检测到 StepFun 开放平台登录状态，部分功能可能受限。");
        println!("如需登录请运行: step login\n");
    }

    if matches!(cli.command, Some(Commands::Setup)) || cli.setup {
        config = setup::run_setup().await?;
    } else if !Config::config_file_exists() && config.api_key.is_empty() {
        println!("No configuration found. Running first-time setup...\n");
        config = setup::run_setup().await?;
    }

    if config.api_key.is_empty() {
        eprintln!("Warning: STEPFUN_API_KEY not set. API calls will fail.");
    }

    let client = ChatClient::new(&config)?;
    let mut registry = build_registry();
    let mcp_path = dirs::home_dir()
        .map(|h| h.join(".step").join("mcp.json"))
        .unwrap_or_else(|| PathBuf::from(".step/mcp.json"));
    match load_mcp_tools(&mcp_path).await {
        Ok(tools) => {
            for tool in tools {
                registry.register(tool);
            }
        }
        Err(e) => eprintln!("Warning: failed to load MCP tools: {}", e),
    }

    let ctx = ToolContext {
        workspace: config
            .workspace
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap()),
        trust: config.trust,
        yolo: config.yolo,
        allow_shell: config.allow_shell,
        job_manager: None,
        search_provider: config.search_provider.clone(),
        search_api_key: config.search_api_key.clone(),
    };

    let mut session = Session::new();
    session.push(Message::system(SYSTEM_PROMPT));

    if let Some(ref prompt) = cli.prompt {
        let user_message = build_headless_user_message(prompt, &cli, &config).await?;
        let approver: AutoApprover = AutoApprover(config.yolo);
        run_headless(
            &client,
            &registry,
            &ctx,
            &approver,
            &mut session,
            user_message,
            &config,
        )
        .await?;
        return Ok(());
    }

    let is_tty = std::io::stdout().is_terminal();
    if is_tty && !cli.no_tui {
        ui::run_tui(client, registry, ctx, session, config).await?;
    } else {
        let approver: ConsoleApprover = ConsoleApprover;
        run_repl(&client, &registry, &ctx, &approver, &mut session, &config).await?;
    }
    Ok(())
}

async fn run_headless(
    client: &ChatClient,
    registry: &ToolRegistry,
    ctx: &ToolContext,
    approver: &dyn chat::approver::Approver,
    session: &mut Session,
    user_message: Message,
    config: &Config,
) -> Result<()> {
    session.push(user_message);
    run_agent_turn(client, registry, ctx, approver, session, config).await?;
    println!();
    if let Ok(dir) = Config::load().and_then(|c| c.sessions_dir()) {
        let _ = session.save(&dir);
    }
    Ok(())
}

async fn build_headless_user_message(prompt: &str, cli: &Cli, config: &Config) -> Result<Message> {
    let workspace = config.workspace.as_ref().unwrap();

    let mut text = prompt.to_string();
    if let Some(audio) = &cli.audio {
        let path = asr::resolve_audio_path(&audio.to_string_lossy(), workspace, config.trust)?;
        let client = AsrClient::from_config(
            &config.base_url,
            &config.api_key,
            cli.asr_model.as_deref().or(config.asr_model.as_deref()),
        );
        let transcript = client.transcribe(&path).await?;
        text.push_str("\n\n[Audio transcript]\n");
        text.push_str(&transcript);
    }

    if cli.image.is_empty() {
        return Ok(Message::user(text));
    }

    if !vision::is_vision_model(&config.model) {
        eprintln!(
            "Warning: model {} may not support images. Use a vision model such as step-1o-turbo-vision.",
            config.model
        );
    }

    let mut image_paths = Vec::new();
    for raw in &cli.image {
        let path = vision::resolve_image_path(&raw.to_string_lossy(), workspace, config.trust)?;
        image_paths.push(path);
    }
    let content = vision::build_user_content(&text, &image_paths)?;
    Ok(Message::user(content))
}

async fn run_repl(
    client: &ChatClient,
    registry: &ToolRegistry,
    ctx: &ToolContext,
    approver: &dyn chat::approver::Approver,
    session: &mut Session,
    config: &Config,
) -> Result<()> {
    println!("step-cli REPL. Type /help for commands, /exit to quit.");
    let mut rl = rustyline::DefaultEditor::new()?;
    let mut pending_images: Vec<PathBuf> = Vec::new();
    loop {
        let readline = rl.readline("step> ");
        match readline {
            Ok(line) => {
                let _ = rl.add_history_entry(&line);
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(cmd) = line.strip_prefix('/') {
                    match handle_slash(cmd, session, config, ctx, &mut pending_images).await? {
                        ReplAction::Continue => continue,
                        ReplAction::Exit => break,
                        ReplAction::Send(msg) => {
                            session.push(msg);
                            print!("Assistant: ");
                            let _ = std::io::stdout().flush();
                            run_agent_turn(client, registry, ctx, approver, session, config)
                                .await?;
                            println!();
                        }
                    }
                } else {
                    let msg =
                        build_repl_user_message(line, ctx, &mut pending_images, config).await?;
                    session.push(msg);
                    print!("Assistant: ");
                    let _ = std::io::stdout().flush();
                    run_agent_turn(client, registry, ctx, approver, session, config).await?;
                    println!();
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}

async fn build_repl_user_message(
    line: &str,
    ctx: &ToolContext,
    pending_images: &mut Vec<std::path::PathBuf>,
    config: &Config,
) -> Result<Message> {
    let (mut text, md_images) = vision::extract_image_paths(line);
    let mut all_image_paths = std::mem::take(pending_images);
    for raw in md_images {
        let path = vision::resolve_image_path(&raw, &ctx.workspace, ctx.trust)?;
        all_image_paths.push(path);
    }
    if all_image_paths.is_empty() {
        return Ok(Message::user(text));
    }
    if !vision::is_vision_model(&config.model) {
        eprintln!(
            "Warning: model {} may not support images. Use a vision model such as step-1o-turbo-vision.",
            config.model
        );
    }
    // Trim whitespace left behind by removed markdown image references.
    text = text.trim().to_string();
    let content = vision::build_user_content(&text, &all_image_paths)?;
    Ok(Message::user(content))
}

enum ReplAction {
    Continue,
    Exit,
    Send(Message),
}

async fn handle_slash(
    cmd: &str,
    session: &mut Session,
    config: &Config,
    ctx: &ToolContext,
    pending_images: &mut Vec<std::path::PathBuf>,
) -> Result<ReplAction> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.first().copied() {
        Some("help") => {
            println!(
                "Commands: /help, /exit, /clear, /save, /sessions, /jobs, /checkpoint, /restore, /login, /logout, /image <path>, /transcribe [--send] <path>"
            );
        }
        Some("exit" | "quit") => return Ok(ReplAction::Exit),
        Some("clear") => {
            session
                .messages
                .retain(|m| m.role == chat::session::Role::System);
            pending_images.clear();
            println!("Cleared conversation history.");
        }
        Some("save") => {
            if let Ok(dir) = config.sessions_dir() {
                session.save(&dir)?;
                println!("Saved session {}", session.id);
            }
        }
        Some("sessions") => {
            if let Ok(dir) = config.sessions_dir() {
                for entry in std::fs::read_dir(dir)?.flatten() {
                    println!("{}", entry.file_name().to_string_lossy());
                }
            }
        }
        Some("image") => {
            if parts.len() < 2 {
                println!("Usage: /image <path>");
            } else {
                let raw = parts[1];
                match vision::resolve_image_path(raw, &ctx.workspace, ctx.trust) {
                    Ok(path) => {
                        pending_images.push(path.clone());
                        println!("Attached image: {}", path.display());
                    }
                    Err(e) => eprintln!("Failed to attach image: {}", e),
                }
            }
        }
        Some("transcribe") => {
            let send = parts.get(1).copied() == Some("--send");
            let path_idx = if send { 2 } else { 1 };
            if parts.len() <= path_idx {
                println!("Usage: /transcribe [--send] <path>");
            } else {
                let raw = parts[path_idx];
                match asr::resolve_audio_path(raw, &ctx.workspace, ctx.trust) {
                    Ok(path) => {
                        let client = AsrClient::from_config(
                            &config.base_url,
                            &config.api_key,
                            config.asr_model.as_deref(),
                        );
                        match client.transcribe(&path).await {
                            Ok(text) => {
                                if send {
                                    return Ok(ReplAction::Send(Message::user(format!(
                                        "[Audio transcript from {}]\n{}",
                                        path.display(),
                                        text
                                    ))));
                                } else {
                                    println!("[Transcript]\n{}", text);
                                }
                            }
                            Err(e) => eprintln!("Transcription failed: {}", e),
                        }
                    }
                    Err(e) => eprintln!("Failed to resolve audio path: {}", e),
                }
            }
        }
        Some("yolo") => {
            println!("Use --yolo at startup to enable auto-approval.");
        }
        Some("login") => match PlatformAuth::login_interactive().await {
            Ok(auth) => {
                let user = auth
                    .username
                    .map(|u| format!(" ({})", u))
                    .unwrap_or_default();
                println!("已登录 StepFun 开放平台{}。", user);
            }
            Err(e) => eprintln!("登录失败: {}", e),
        },
        Some("logout") => match PlatformAuth::logout() {
            Ok(()) => println!("已退出 StepFun 开放平台登录。"),
            Err(e) => eprintln!("退出登录失败: {}", e),
        },
        _ => println!("Unknown command. Use /help."),
    }
    Ok(ReplAction::Continue)
}

async fn run_agent_turn(
    client: &ChatClient,
    registry: &ToolRegistry,
    ctx: &ToolContext,
    approver: &dyn chat::approver::Approver,
    session: &mut Session,
    config: &Config,
) -> Result<String> {
    let schemas = registry.schemas();
    let executor = Executor::new(registry, ctx, approver);

    for _round in 0..config.max_rounds {
        let model = client.model();
        if chat::context::compress_if_needed(&mut session.messages, model, config.context_threshold)
        {
            tracing::info!(
                "Context compressed to stay under {} threshold for {}",
                config.context_threshold,
                model
            );
        }

        let request = chat::client::ChatRequest::new(
            model.to_string(),
            session.messages.clone(),
            schemas.clone(),
            None,
            None,
        );
        let mut content = String::new();
        let mut tool_calls: Option<Vec<chat::session::ToolCall>> = None;

        let mut stream = client.stream(request);
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::Start => {}
                StreamEvent::ContentDelta(delta) => {
                    content.push_str(&delta);
                    print_flush(&delta);
                }
                StreamEvent::Done => {
                    session.push(Message::assistant(content.as_str()));
                    return Ok(content);
                }
                StreamEvent::ToolCalls(calls) => {
                    tool_calls = Some(calls);
                    break;
                }
            }
        }

        if let Some(calls) = tool_calls.take() {
            println!();
            session.push(Message::assistant_with_tools(calls.clone()));
            let results = executor.execute(calls).await?;
            for (id, result) in results {
                session.push(Message::tool(id, result));
            }
        } else {
            session.push(Message::assistant(content.as_str()));
            return Ok(content);
        }
    }

    bail!("exceeded maximum agent rounds");
}

async fn run_doctor(config: &Config) -> Result<()> {
    println!("Configuration:");
    if let Some(path) = Config::path_display() {
        println!("  config_file: {}", path);
    }
    println!("  base_url: {}", config.base_url);
    println!("  model: {}", config.model);
    println!(
        "  api_key: {}",
        if config.api_key.is_empty() {
            "NOT SET"
        } else {
            "set"
        }
    );
    println!("  workspace: {:?}", config.workspace);
    println!("  yolo: {}", config.yolo);

    let auth = PlatformAuth::load()?;
    println!(
        "  platform_login: {}",
        if auth.is_authenticated() { "yes" } else { "no" }
    );
    if let Some(user) = auth.username {
        println!("  platform_user: {}", user);
    }

    if config.api_key.is_empty() {
        return Ok(());
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/models", config.base_url))
        .header("Authorization", format!("Bearer {}", config.api_key))
        .send()
        .await
        .context("failed to contact StepFun API")?;
    println!("  /models status: {}", resp.status());
    if resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        println!("  response length: {} bytes", body.len());
    }
    Ok(())
}
