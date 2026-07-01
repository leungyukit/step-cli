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

### Prebuilt binaries (recommended)

Download the latest release for your platform from the
[GitHub Releases](https://github.com/liangyj/step-cli/releases) page and place
the binary in a directory on your `PATH`.

```bash
# macOS / Linux example
VERSION=$(curl -s https://api.github.com/repos/liangyj/step-cli/releases/latest | grep '"tag_name":' | cut -d'"' -f4)
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
curl -LO "https://github.com/liangyj/step-cli/releases/download/${VERSION}/step-${ARCH}-apple-${OS}.tar.gz"
# Adjust the archive name to match your platform, then extract and install
tar xzf "step-${ARCH}-apple-${OS}.tar.gz"
chmod +x step
mv step /usr/local/bin/step
```

### cargo install

If you already have the Rust toolchain installed:

```bash
cargo install --path .
# Or install directly from crates.io once published:
# cargo install step-cli
```

### Build from source

```bash
git clone https://github.com/liangyj/step-cli.git
cd step-cli
cargo build --release
# The binary is at target/release/step
```

## Configuration

### 首次使用引导

第一次运行 `step` 时，会进入 API 配置向导（登录 StepFun 开放平台是可选的）：

```bash
step
# 1. 选择 Step API（按量） 或 Step Plan（套餐）
# 2. 输入 API Key（支持任意格式，不再限制 sk- 开头）
# 3. 选择模型 ID
# 4. 完成，开始对话
```

也可以随时手动运行：

```bash
# 配置 API
step setup
# 或
step --setup

# 登录 / 退出 StepFun 开放平台
step login
step logout
```

### 环境变量 / 配置文件

```bash
export STEPFUN_API_KEY="你的 API Key"
```

或编辑 `~/.step/config.toml` 手动配置（配置文件路径可通过 `step doctor` 查看）。

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
- `/login` — log in to the StepFun open platform
- `/logout` — log out from the StepFun open platform

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

## Platform Login (Optional)

step-cli 支持登录 [StepFun 开放平台](https://platform.stepfun.com)，但**不再强制要求**。未登录只会显示提示，不会阻止你配置模型和使用 API。

登录信息保存在 `~/.step/state/auth.json`。

### 登录 / 退出

```bash
step login   # 登录 StepFun 开放平台
step logout  # 退出登录
```

在 TUI / REPL 中也可以随时输入 `/logout` 退出登录。

### 浏览器自动登录

选择浏览器登录时，step-cli 会启动系统上的 Chrome/Chromium，打开 StepFun 开放平台登录页。你在浏览器窗口完成登录后，程序会自动读取 session cookie 并保存，无需手动复制粘贴。

如果系统未安装 Chrome/Chromium，或自动获取失败，会自动回退到手动粘贴 cookie/token 的方式。

## License

MIT
