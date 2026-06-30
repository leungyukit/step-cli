use crate::chat::approver::Approver;
use crate::chat::client::{ChatClient, StreamEvent};
use crate::chat::executor::Executor;
use crate::chat::session::{Message, Role, Session, ToolCall};
use crate::chat::tools::{ToolContext, ToolRegistry};
use crate::config::Config;
use crate::runtime::checkpoints::CheckpointManager;
use crate::runtime::jobs::{JobManager, JobStatus};
use crate::skills::loader::SkillRegistry;
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
enum DisplayMessage {
    User(String),
    Assistant(String),
    Tool { name: String, result: String },
    Info(String),
    Error(String),
}

#[derive(Debug)]
#[allow(dead_code)]
enum AppEvent {
    Terminal(Event),
    AssistantStart,
    AssistantDelta(String),
    AssistantDone,
    ToolCalls(Vec<ToolCall>),
    RequestApproval {
        id: String,
        name: String,
        args: String,
        respond: oneshot::Sender<bool>,
    },
    ToolResult {
        id: String,
        name: String,
        result: String,
    },
    ToolCallsComplete,
    Error(String),
}

struct TuiApp {
    client: ChatClient,
    registry: ToolRegistry,
    ctx: ToolContext,
    config: Config,
    session: Session,
    messages: Vec<DisplayMessage>,
    input: String,
    input_mode: InputMode,
    scroll: usize,
    status: String,
    popup: Popup,
    running: bool,
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    approval_responders: HashMap<String, oneshot::Sender<bool>>,
    checkpoint_manager: CheckpointManager,
    skill_registry: SkillRegistry,
    pending_tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Command,
}

#[derive(Debug, Default)]
struct Popup {
    visible: bool,
    title: String,
    body: String,
    respond_id: Option<String>,
}

pub async fn run_tui(
    client: ChatClient,
    registry: ToolRegistry,
    ctx: ToolContext,
    session: Session,
    config: Config,
) -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, rx) = mpsc::unbounded_channel();
    let app = TuiApp::new(client, registry, ctx, session, config, tx, rx)?;
    let result = app.run(&mut terminal).await;

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    result
}

impl TuiApp {
    fn new(
        client: ChatClient,
        registry: ToolRegistry,
        ctx: ToolContext,
        session: Session,
        config: Config,
        tx: mpsc::UnboundedSender<AppEvent>,
        rx: mpsc::UnboundedReceiver<AppEvent>,
    ) -> Result<Self> {
        let checkpoints_dir = config.state_dir()?.join("checkpoints");
        let jobs_state = config.state_dir()?.join("background_jobs.json");
        let job_manager = JobManager::new(jobs_state)?;
        let global_skills = dirs::home_dir()
            .map(|h| h.join(".step").join("skills"))
            .unwrap_or_else(|| PathBuf::from(".step/skills"));
        let skill_registry = SkillRegistry::load(&global_skills, config.workspace.as_deref())?;
        let mut ctx = ctx;
        ctx.job_manager = Some(Arc::new(tokio::sync::Mutex::new(job_manager)));
        let mut session = session;
        if let Some(extra) = skill_registry.system_prompt_extras().into() {
            // Prepend skill info to the system prompt if present.
            if let Some(sys) = session.messages.iter_mut().find(|m| m.role == Role::System) {
                if let Some(content) = &mut sys.content {
                    content.push_str(&extra);
                }
            }
        }
        Ok(Self {
            client,
            registry,
            ctx,
            config,
            session,
            messages: Vec::new(),
            input: String::new(),
            input_mode: InputMode::Normal,
            scroll: 0,
            status: "Press Ctrl+C or type /exit to quit".to_string(),
            popup: Popup::default(),
            running: true,
            tx,
            rx,
            approval_responders: HashMap::new(),
            checkpoint_manager: CheckpointManager::new(checkpoints_dir)?,
            skill_registry,
            pending_tool_calls: Vec::new(),
        })
    }

    async fn run<B: ratatui::backend::Backend>(mut self, terminal: &mut Terminal<B>) -> Result<()> {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut stream = crossterm::event::EventStream::new();
            while let Some(Ok(event)) = stream.next().await {
                if tx.send(AppEvent::Terminal(event)).is_err() {
                    break;
                }
            }
        });

        let mut last_tick = tokio::time::Instant::now();
        while self.running {
            terminal.draw(|f| self.draw(f))?;

            let timeout = tokio::time::Duration::from_millis(100);
            let event = tokio::time::timeout(timeout, self.rx.recv()).await;
            match event {
                Ok(Some(ev)) => self.handle_event(ev).await?,
                Ok(None) => break,
                Err(_) => {
                    // periodic tick could refresh running jobs here
                    let _ = last_tick;
                    last_tick = tokio::time::Instant::now();
                }
            }
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        // Chat area
        let chat_block = Block::default().title(" step-cli ").borders(Borders::ALL);
        let chat_inner = chat_block.inner(chunks[0]);
        frame.render_widget(chat_block, chunks[0]);

        let lines: Vec<Line> = self
            .messages
            .iter()
            .flat_map(|m| self.format_message(m))
            .collect();
        let paragraph = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll as u16, 0));
        frame.render_widget(paragraph, chat_inner);

        // Input area
        let input_title = if self.input_mode == InputMode::Command {
            " Command (press Esc to cancel) "
        } else {
            " Input "
        };
        let input_block = Block::default().title(input_title).borders(Borders::ALL);
        let input_text = if self.input_mode == InputMode::Command && !self.input.starts_with('/') {
            format!("/{}", self.input)
        } else {
            self.input.clone()
        };
        let input_para = Paragraph::new(input_text)
            .block(input_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(input_para, chunks[1]);

        // Status bar
        let status = Paragraph::new(Line::from(vec![
            Span::styled("model: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&self.config.model),
            Span::raw(" | "),
            Span::raw(&self.status),
        ]));
        frame.render_widget(status, chunks[2]);

        // Popup
        if self.popup.visible {
            let popup_area = self.centered_rect(60, 40, area);
            frame.render_widget(Clear, popup_area);
            let popup_block = Block::default()
                .title(self.popup.title.clone())
                .borders(Borders::ALL)
                .style(Style::default().bg(Color::Black));
            let para = Paragraph::new(self.popup.body.clone())
                .block(popup_block)
                .wrap(Wrap { trim: false });
            frame.render_widget(para, popup_area);
        }
    }

    fn format_message<'a>(&self, msg: &'a DisplayMessage) -> Vec<Line<'a>> {
        match msg {
            DisplayMessage::User(text) => {
                let mut lines = vec![Line::from(Span::styled(
                    "You:",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))];
                for l in text.lines() {
                    lines.push(Line::from(Span::raw(l)));
                }
                lines
            }
            DisplayMessage::Assistant(text) => {
                let mut lines = vec![Line::from(Span::styled(
                    "Assistant:",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ))];
                for l in text.lines() {
                    lines.push(Line::from(Span::raw(l)));
                }
                lines
            }
            DisplayMessage::Tool { name, result } => {
                vec![
                    Line::from(Span::styled(
                        format!("  → tool {}: ", name),
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(Span::raw(result.lines().next().unwrap_or(""))),
                ]
            }
            DisplayMessage::Info(text) => vec![Line::from(Span::styled(
                text.clone(),
                Style::default().fg(Color::Blue),
            ))],
            DisplayMessage::Error(text) => vec![Line::from(Span::styled(
                text.clone(),
                Style::default().fg(Color::Red),
            ))],
        }
    }

    fn centered_rect(&self, percent_x: u16, percent_y: u16, r: Rect) -> Rect {
        let popup_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ])
            .split(r);
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(popup_layout[1])[1]
    }

    async fn handle_event(&mut self, event: AppEvent) -> Result<()> {
        match event {
            AppEvent::Terminal(Event::Resize(_w, _h)) => {}
            AppEvent::Terminal(Event::Key(key)) => self.handle_key(key).await?,
            AppEvent::AssistantStart => {
                self.messages.push(DisplayMessage::Assistant(String::new()));
                self.status = "Assistant is thinking...".to_string();
            }
            AppEvent::AssistantDelta(delta) => {
                if let Some(DisplayMessage::Assistant(text)) = self.messages.last_mut() {
                    text.push_str(&delta);
                } else {
                    self.messages.push(DisplayMessage::Assistant(delta));
                }
                self.scroll = self.messages.len().saturating_sub(1);
            }
            AppEvent::AssistantDone => {
                self.status = "Ready".to_string();
                if let Some(DisplayMessage::Assistant(text)) = self.messages.last() {
                    let text = text.clone();
                    self.session.push(Message::assistant(text));
                }
            }
            AppEvent::ToolCalls(calls) => {
                self.pending_tool_calls = calls.clone();
                self.process_tool_calls(calls).await?;
            }
            AppEvent::RequestApproval {
                id,
                name,
                args,
                respond,
            } => {
                self.approval_responders.insert(id.clone(), respond);
                self.popup = Popup {
                    visible: true,
                    title: format!("Approve {}?", name),
                    body: args,
                    respond_id: Some(id),
                };
            }
            AppEvent::ToolResult { id, name, result } => {
                self.session.push(Message::tool(id, &result));
                self.messages.push(DisplayMessage::Tool { name, result });
                self.popup.visible = false;
            }
            AppEvent::ToolCallsComplete => {
                self.pending_tool_calls.clear();
                self.run_agent_turn().await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.popup.visible {
            if let Some(id) = self.popup.respond_id.take() {
                let approved = matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y'));
                if let Some(responder) = self.approval_responders.remove(&id) {
                    let _ = responder.send(approved);
                }
                self.popup.visible = false;
                return Ok(());
            }
        }
        match self.input_mode {
            InputMode::Normal => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.running = false;
                }
                KeyCode::Enter => {
                    let text = self.input.trim().to_string();
                    self.input.clear();
                    if text.is_empty() {
                        return Ok(());
                    }
                    if let Some(cmd) = text.strip_prefix('/') {
                        self.handle_command(cmd).await?;
                    } else {
                        self.submit_user(text).await?;
                    }
                }
                KeyCode::Char(c) => self.input.push(c),
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Esc => {
                    self.input.clear();
                }
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(5),
                _ => {}
            },
            InputMode::Command => match key.code {
                KeyCode::Enter => {
                    let text = self.input.trim().to_string();
                    self.input.clear();
                    self.input_mode = InputMode::Normal;
                    if !text.is_empty() {
                        let cmd = text.strip_prefix('/').unwrap_or(&text);
                        self.handle_command(cmd).await?;
                    }
                }
                KeyCode::Esc => {
                    self.input.clear();
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char(c) => self.input.push(c),
                KeyCode::Backspace => {
                    self.input.pop();
                }
                _ => {}
            },
        }
        Ok(())
    }

    async fn submit_user(&mut self, text: String) -> Result<()> {
        self.messages.push(DisplayMessage::User(text.clone()));
        self.session.push(Message::user(text));
        self.run_agent_turn().await?;
        Ok(())
    }

    async fn handle_command(&mut self, cmd: &str) -> Result<()> {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        match parts.first().copied() {
            Some("exit" | "quit") => self.running = false,
            Some("help") => {
                self.messages.push(DisplayMessage::Info(
                    "Commands: /exit, /clear, /save, /sessions, /jobs, /checkpoint [name], /restore <id>, /skills, /skill <name>, /yolo".to_string(),
                ));
            }
            Some("clear") => {
                self.session.messages.retain(|m| m.role == Role::System);
                self.messages.clear();
            }
            Some("save") => {
                self.session.save(&self.config.sessions_dir()?)?;
                self.messages.push(DisplayMessage::Info(format!(
                    "Saved session {}",
                    self.session.id
                )));
            }
            Some("sessions") => {
                let dir = self.config.sessions_dir()?;
                for entry in std::fs::read_dir(dir)?.flatten() {
                    self.messages.push(DisplayMessage::Info(
                        entry.file_name().to_string_lossy().to_string(),
                    ));
                }
            }
            Some("checkpoint") => {
                let name = parts.get(1).map(|s| s.to_string());
                let id =
                    self.checkpoint_manager
                        .create(&self.session, &self.ctx.workspace, name)?;
                self.messages
                    .push(DisplayMessage::Info(format!("Checkpoint created: {}", id)));
            }
            Some("restore") => {
                if let Some(id) = parts.get(1) {
                    let session = self.checkpoint_manager.restore(id, &self.ctx.workspace)?;
                    self.session = session;
                    self.messages = Vec::new();
                    self.messages
                        .push(DisplayMessage::Info(format!("Restored checkpoint {}", id)));
                } else {
                    self.messages.push(DisplayMessage::Error(
                        "Usage: /restore <checkpoint-id>".to_string(),
                    ));
                }
            }
            Some("jobs") => {
                if parts.len() > 1 && parts[1] == "cancel" {
                    if let Some(id) = parts.get(2) {
                        if let Some(jm) = self.ctx.job_manager.as_ref() {
                            jm.lock().await.cancel(id).await?;
                        }
                        self.messages
                            .push(DisplayMessage::Info(format!("Cancelled job {}", id)));
                    }
                } else {
                    let jobs = if let Some(jm) = self.ctx.job_manager.as_ref() {
                        jm.lock()
                            .await
                            .list()
                            .into_iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    for job in jobs {
                        let status = match &job.status {
                            JobStatus::Running => "running".to_string(),
                            JobStatus::Completed { exit_code } => {
                                format!("completed {:?}", exit_code)
                            }
                            JobStatus::Failed { reason } => format!("failed: {}", reason),
                            JobStatus::Orphaned { reason } => format!("orphaned: {}", reason),
                        };
                        self.messages.push(DisplayMessage::Info(format!(
                            "{} {} -> {}",
                            job.id, job.command, status
                        )));
                    }
                }
            }
            Some("skills") => {
                for skill in self.skill_registry.list() {
                    self.messages.push(DisplayMessage::Info(skill.name.clone()));
                }
            }
            Some("skill") => {
                if let Some(name) = parts.get(1) {
                    if let Some(skill) = self.skill_registry.get(name) {
                        self.messages.push(DisplayMessage::Info(format!(
                            "Skill {}:\n{}",
                            skill.name, skill.content
                        )));
                    } else {
                        self.messages
                            .push(DisplayMessage::Error(format!("Skill {} not found", name)));
                    }
                }
            }
            Some("yolo") => {
                self.ctx.yolo = !self.ctx.yolo;
                self.messages.push(DisplayMessage::Info(format!(
                    "YOLO mode: {}",
                    self.ctx.yolo
                )));
            }
            Some("trust") => {
                self.ctx.trust = !self.ctx.trust;
                self.messages.push(DisplayMessage::Info(format!(
                    "Trust mode: {}",
                    self.ctx.trust
                )));
            }
            Some(":" | "cmd") => {
                self.input_mode = InputMode::Command;
            }
            _ => {
                self.messages
                    .push(DisplayMessage::Error(format!("Unknown command: /{}", cmd)));
            }
        }
        Ok(())
    }

    async fn process_tool_calls(&mut self, calls: Vec<ToolCall>) -> Result<()> {
        if calls.is_empty() {
            let _ = self.tx.send(AppEvent::ToolCallsComplete);
            return Ok(());
        }
        let tx = self.tx.clone();
        let registry = self.registry.clone();
        let ctx = self.ctx.clone();
        tokio::spawn(async move {
            let approver = TuiApprover::new(tx.clone());
            let executor = Executor::new(&registry, &ctx, &approver);
            match executor.execute(calls.clone()).await {
                Ok(results) => {
                    for (call, (id, result)) in calls.iter().zip(results) {
                        let _ = tx.send(AppEvent::ToolResult {
                            id,
                            name: call.function.name.clone(),
                            result,
                        });
                    }
                    let _ = tx.send(AppEvent::ToolCallsComplete);
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(e.to_string()));
                }
            }
        });
        Ok(())
    }

    async fn run_agent_turn(&mut self) -> Result<()> {
        let client = self.client.clone();
        let registry = self.registry.clone();
        let session = self.session.clone();
        let tx = self.tx.clone();
        let schemas = registry.schemas();
        let model = client.model().to_string();
        let max_rounds = self.config.max_rounds;

        let _ = max_rounds;
        tokio::spawn(async move {
            let current_session = session;
            let request = crate::chat::client::ChatRequest::new(
                model.clone(),
                current_session.messages.clone(),
                schemas.clone(),
                None,
                None,
            );
            let _ = tx.send(AppEvent::AssistantStart);
            let mut tool_calls: Option<Vec<ToolCall>> = None;
            let mut stream = client.stream(request);
            while let Some(event) = stream.next().await {
                match event {
                    Ok(StreamEvent::Start) => {}
                    Ok(StreamEvent::ContentDelta(delta)) => {
                        let _ = tx.send(AppEvent::AssistantDelta(delta));
                    }
                    Ok(StreamEvent::Done) => {
                        let _ = tx.send(AppEvent::AssistantDone);
                        return;
                    }
                    Ok(StreamEvent::ToolCalls(calls)) => {
                        tool_calls = Some(calls);
                        break;
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(e.to_string()));
                        return;
                    }
                }
            }

            if let Some(calls) = tool_calls {
                let _ = tx.send(AppEvent::ToolCalls(calls));
            } else {
                let _ = tx.send(AppEvent::AssistantDone);
            }
        });
        Ok(())
    }
}

struct TuiApprover {
    tx: mpsc::UnboundedSender<AppEvent>,
}

impl TuiApprover {
    fn new(tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl Approver for TuiApprover {
    async fn approve(&self, call_id: &str, tool_name: &str, args: &str) -> bool {
        let (respond_tx, respond_rx) = oneshot::channel();
        let _ = self.tx.send(AppEvent::RequestApproval {
            id: call_id.to_string(),
            name: tool_name.to_string(),
            args: args.to_string(),
            respond: respond_tx,
        });
        respond_rx.await.unwrap_or(false)
    }
}
