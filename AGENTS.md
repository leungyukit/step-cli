# Project Instructions

## Project Type: Rust

## Commands

- Build: `cargo build`
- Test: `cargo test`
- Run: `cargo run -- <args>`
- Check: `cargo check`
- Format: `cargo fmt`
- Lint: `cargo clippy -- -D warnings`

## Project: step-cli

A Rust CLI coding agent for StepFun models.

## Modules

- `src/chat/` — LLM client, SSE streaming, messages, tools registry, executor, approval.
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
