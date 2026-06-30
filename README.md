# step-cli

A Rust-based AI coding agent CLI powered by [StepFun](https://www.stepfun.com) models.

Designed to feel like Claude Code / Kimi Code CLI / MiniMax-CLI: multi-turn chat,
tool use, file editing, shell execution, session persistence, TUI, checkpoints,
skills, background jobs, and MCP.

## Features

- **OpenAI-compatible StepFun API** support (`/v1/chat/completions`).
- **Streaming responses** with function/tool calling.
- **TUI mode** by default (`--no-tui` for line REPL).
- **Headless mode** (`-p`) for one-shot prompts.
- **Built-in tools**: read/write/edit files, list directory, glob, grep, shell execution.
- **Background shell jobs** with `/jobs` and `/jobs cancel <id>`.
- **Checkpoints**: `/checkpoint [name]` and `/restore <id>` to save/restore session + workspace files.
- **Skills**: auto-load `SKILL.md` directories from `~/.step/skills` and project `.agents/skills`.
- **MCP**: load external tool servers via `~/.step/mcp.json` (stdio transport).
- **Workspace boundary** and `--trust` / `--yolo` modes.
- **Session persistence** under `~/.step/sessions/`.
- **`doctor`** command for quick config/API diagnostics.

## Install

```bash
git clone https://github.com/liangyj/step-cli.git
cd step-cli
cargo build --release
# The binary is at target/release/step
```

## Configuration

Set your StepFun API key:

```bash
export STEPFUN_API_KEY="sk-..."
```

Or create `~/.step/config.toml` from `config.example.toml`.

Useful environment variables:

- `STEPFUN_API_KEY` — API key (overrides config file).
- `STEP_BASE_URL` — default `https://api.stepfun.com/step_plan/v1`.
- `STEP_MODEL` — default `step-2-16k`.
- `STEP_WORKSPACE` — workspace directory.
- `STEP_ALLOW_SHELL=1` — allow shell execution.
- `STEP_YOLO=1` — auto-approve all tool calls.

### MCP configuration

Create `~/.step/mcp.json`:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/to/allow"]
    }
  }
}
```

## Usage

```bash
# One-shot prompt
step -p "Explain this codebase"

# Start TUI (default)
step

# Line-based REPL
step --no-tui

# Use a different model / base URL
step -m step-3.7-flash --base-url https://api.stepfun.com/step_plan/v1

# YOLO mode (auto-approve all tools)
step --yolo -p "Refactor src/main.rs"

# Diagnose setup
step doctor
```

### TUI / REPL commands

- `/help` — show commands
- `/exit` or `/quit` — leave
- `/clear` — clear conversation history
- `/save` — save session
- `/sessions` — list saved sessions
- `/checkpoint [name]` — create a checkpoint
- `/restore <id>` — restore a checkpoint
- `/jobs` — list background jobs
- `/jobs cancel <id>` — cancel a job
- `/skills` — list loaded skills
- `/skill <name>` — view skill content
- `/yolo` — toggle auto-approval
- `/trust` — toggle workspace trust

## Safety

By default, file writes and shell commands require explicit approval (`y/N`) in
the TUI or REPL. `--yolo` disables prompts and is dangerous; only use in trusted
environments.

## Development

```bash
cargo build
cargo test
cargo clippy -- -D warnings
```

## License

MIT
