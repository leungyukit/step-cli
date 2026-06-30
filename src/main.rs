pub mod chat;
pub mod cli;
pub mod config;
pub mod runtime;
pub mod setup;
pub mod skills;
pub mod tools;
pub mod ui;

use anyhow::{bail, Context, Result};
use chat::approver::{AutoApprover, ConsoleApprover};
use chat::client::{ChatClient, StreamEvent};
use chat::executor::Executor;
use chat::session::{Message, Session};
use chat::tools::{ToolContext, ToolRegistry};
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

const SYSTEM_PROMPT: &str = r#"You are step-cli, an AI coding assistant.
You have access to a workspace on the local machine and a set of tools.
Only use tools when necessary. Prefer reading before writing.
When editing files, use the exact edit_file tool with old_string/new_string.
Do not run destructive commands. The user must approve shell commands and file writes unless --yolo is enabled.
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
    };

    let mut session = Session::new();
    session.push(Message::system(SYSTEM_PROMPT));

    if let Some(prompt) = cli.prompt {
        let approver: AutoApprover = AutoApprover(config.yolo);
        run_headless(
            &client,
            &registry,
            &ctx,
            &approver,
            &mut session,
            &prompt,
            config.max_rounds,
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
    prompt: &str,
    max_rounds: usize,
) -> Result<()> {
    session.push(Message::user(prompt));
    run_agent_turn(client, registry, ctx, approver, session, max_rounds).await?;
    println!();
    if let Ok(dir) = Config::load().and_then(|c| c.sessions_dir()) {
        let _ = session.save(&dir);
    }
    Ok(())
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
                    match handle_slash(cmd, session, config).await? {
                        ReplAction::Continue => continue,
                        ReplAction::Exit => break,
                    }
                } else {
                    session.push(Message::user(line));
                    print!("Assistant: ");
                    let _ = std::io::stdout().flush();
                    run_agent_turn(client, registry, ctx, approver, session, config.max_rounds)
                        .await?;
                    println!();
                }
            }
            Err(_) => break,
        }
    }
    Ok(())
}

enum ReplAction {
    Continue,
    Exit,
}

async fn handle_slash(cmd: &str, session: &mut Session, config: &Config) -> Result<ReplAction> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.first().copied() {
        Some("help") => {
            println!(
                "Commands: /help, /exit, /clear, /save, /sessions, /jobs, /checkpoint, /restore"
            );
        }
        Some("exit" | "quit") => return Ok(ReplAction::Exit),
        Some("clear") => {
            session
                .messages
                .retain(|m| m.role == chat::session::Role::System);
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
        Some("yolo") => {
            println!("Use --yolo at startup to enable auto-approval.");
        }
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
    max_rounds: usize,
) -> Result<String> {
    let schemas = registry.schemas();
    let executor = Executor::new(registry, ctx, approver);

    for _round in 0..max_rounds {
        let request = chat::client::ChatRequest::new(
            client.model().to_string(),
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
                    session.push(Message::assistant(&content));
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
            session.push(Message::assistant(&content));
            return Ok(content);
        }
    }

    bail!("exceeded maximum agent rounds");
}

async fn run_doctor(config: &Config) -> Result<()> {
    println!("Configuration:");
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
