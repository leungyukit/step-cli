<!-- From: /Users/liangyj/workspace/step-cli/AGENTS.md -->
# Project Instructions

## Project Type: Rust

## Commands

- Build: `cargo build`
- Test: `cargo test`
- Run: `cargo run -- <args>`
- Check: `cargo check`
- Format: `cargo fmt`
- Lint: `cargo clippy -- -D warnings`
- Release build: `cargo build --release`
- Local release archive: `./scripts/release.sh [target]`

## Project: step-cli

A Rust CLI coding agent for StepFun models.

## Modules

- `src/auth.rs` — StepFun open platform login state management.
- `src/chat/` — LLM client, SSE streaming, messages, tools registry, executor, approval, vision, ASR.
- `src/tools/` — built-in tools (fs, shell) and MCP client.
- `src/ui/` — ratatui TUI and REPL dispatcher.
- `src/runtime/` — background jobs and checkpoints.
- `src/skills/` — `SKILL.md` loader.
- `src/cli.rs`, `src/config.rs` — argument parsing and configuration.

## Documentation

See `README.md` for user-facing documentation.

## Guidelines

- Keep changes focused and minimal.
- Follow Rust idioms and existing module structure.
- Write unit/integration tests for new functionality.
- Maintain `cargo clippy -- -D warnings` and `cargo test` clean.
- Update `README.md` and `AGENTS.md` when behavior changes.

## Important Notes

- The CLI communicates with StepFun's OpenAI-compatible `/v1/chat/completions` endpoint.
- Tools are async and registered in `src/chat/tools.rs`.
- Workspace boundaries are enforced by file tools unless `--trust` or `--yolo` is set.
- The TUI (`step`) is the default when stdout is a terminal; use `--no-tui` for the line REPL.
- StepFun open platform login is optional (`step login`).
- Browser login uses `chromiumoxide` to launch Chrome/Chromium and automatically capture session cookies.
- API keys are no longer required to start with `sk-`; any non-empty key is accepted.
- Context compression: when `session.messages` tokens exceed `context_threshold * model_context_limit`, the oldest non-system messages are dropped and a system notice is inserted before the LLM call. System prompts are always preserved.
- Web search: a `web_search` tool is available when configured. Supported providers: `serper`, `tavily` (both require API key), and `duckduckgo` (free HTML scraping). The model cannot browse directly; it calls this tool and receives formatted snippets/URLs.
- Vision support: images can be attached to user messages via `--image` / `-i`, `/image <path>`, or Markdown image syntax `![alt](path)`. Local images are base64-encoded into OpenAI-style `image_url` content parts. Use a vision-capable model such as `step-1o-turbo-vision`.
- ASR support: audio files can be transcribed with StepFun `stepaudio-2.5-asr` via the `transcribe` subcommand, `--audio` / `-a`, or `/transcribe [--send] <path>`. The SSE endpoint is `{base_url}/audio/asr/sse`.
- Intent routing: the system prompt instructs the model to autonomously choose among (1) direct answer, (2) local file lookup, (3) web search, (4) vision/image analysis, and (5) audio transcription, and to combine them when useful.
- Supported chat/vision models include `step-3.7-flash`, `step-3.5-flash`, `step-3.5-flash-2603` (256K context), `step-1o-turbo-vision`, `step-audio-2.5-chat`, and the legacy `step-1-*` / `step-2-*` models. ASR/TTS/image models are not supported by the chat endpoint.

## Agent Query Strategy

- When answering questions or investigating issues, prefer autonomous judgment to decide between local file search, web search, or a combination of both.
- Only follow an explicit search source preference when the user clearly requests it (e.g., "search the web", "look at local files only", "do not use the internet").
- For step-cli specific implementation details, local codebase queries usually take priority unless external/upstream information is clearly needed.

## Releasing

1. Bump the version in `Cargo.toml` if needed.
2. Ensure `cargo test` and `cargo clippy -- -D warnings` pass.
3. Tag the release: `git tag -a v0.1.0 -m "Release v0.1.0" && git push origin v0.1.0`.
4. The `.github/workflows/release.yml` action will build binaries for Linux (x86_64, aarch64), macOS (x86_64, Apple Silicon), and Windows (x86_64), then create a GitHub Release with the archives.
5. Locally, `./scripts/release.sh` can build a release archive for the host platform.
