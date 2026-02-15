# Changelog

All notable changes to LocalGPT are documented in this file.

## [Unreleased]

No unreleased changes.

## [0.1.3] - 2026-02-12

A major release focused on security hardening, new provider support, and the Telegram bot interface.

### Added

- **Telegram bot interface** with one-time pairing auth, slash commands, streaming responses with debounced edits, and full tool support. Runs as a background task inside the daemon. ([#15](https://github.com/localgpt-app/localgpt/pull/15))
- **Telegram HTML rendering** for agent responses with markdown-to-HTML conversion. ([#16](https://github.com/localgpt-app/localgpt/pull/16))
- **GLM (Z.AI) provider** support, adding Z.AI's GLM models as a new LLM backend. ([#21](https://github.com/localgpt-app/localgpt/pull/21))
- **Security policy module** with HMAC signing and tamper-detecting audit chain.
- **Kernel-enforced shell sandbox** for LLM-issued commands using macOS Seatbelt and Linux Landlock/seccomp.
- **Prompt injection defenses** with per-turn injection, suspicious-content warnings surfaced to users, and configurable strict policy mode.
- **Windows build support** by gating Unix-only sandbox and nix APIs behind `cfg(unix)`.

### Changed

- Renamed `localgpt security` CLI subcommand to `localgpt md`.
- Updated `LocalGPT.md` init template to match standing-instructions framing.
- Upgraded all dependencies to latest versions.

### Fixed

- Security block incorrectly sent as a prompt to Claude CLI instead of as a user message.
- Clippy warnings for Rust 1.93.
- Landlock/seccompiler API usage updated for latest crate versions.

### Contributors

Thanks to **[@haraldh](https://github.com/haraldh)** for building the Telegram bot interface and HTML rendering, and **[@austingreisman](https://github.com/austingreisman)** for adding GLM (Z.AI) provider support!

## [0.1.2] - 2026-02-09

This release enables tool calling for Ollama and OpenAI-compatible providers, and improves memory search quality.

### Added

- **Ollama tool calling support**, allowing Ollama models to execute agent tools. ([#14](https://github.com/localgpt-app/localgpt/pull/14))
- **Desktop feature flag** for headless builds (compile without GUI dependencies).
- GitHub Actions CI workflow with license audit via `cargo-deny`.

### Fixed

- **OpenAI provider tools** were silently dropped during streaming — the default `chat_stream` fallback now forwards tools and handles `ToolCalls` responses correctly. ([#11](https://github.com/localgpt-app/localgpt/pull/11))
- **Memory search** improved with token AND matching and rank-based scoring for more relevant results. ([#10](https://github.com/localgpt-app/localgpt/pull/10))
- Linux desktop builds now include x11 and Wayland features.

### Contributors

Thanks to **[@JarvisDeLaAri](https://github.com/JarvisDeLaAri)** for enabling Ollama tool calling, and **[@Ax73](https://github.com/Ax73)** for fixing OpenAI provider tool support!

## [0.1.1] - 2026-02-07

Introduces the desktop GUI, GGUF embedding support, and workspace concurrency safety.

### Added

- **Desktop GUI** built with egui, providing a native app experience.
- **GGUF embedding support** via llama.cpp for fully local semantic search.
- **Streaming tool details and slash commands** in the egui and web UIs.
- **Concurrency protections** for workspace file access.

### Fixed

- UTF-8 boundary panics in memory search snippets resolved; indexing simplified.

## [0.1.0] - 2026-02-04

Initial release of LocalGPT — a local-only AI assistant with persistent markdown-based memory.

### Added

- **Interactive CLI chat** with streaming responses and tool execution.
- **Multi-provider LLM support**: Anthropic, OpenAI, Ollama, and Claude CLI.
- **Markdown-based memory system** with `MEMORY.md`, daily logs, and `HEARTBEAT.md`.
- **Semantic search** using SQLite FTS5 and local embeddings via fastembed.
- **Autonomous heartbeat** runner for background task execution on configurable intervals.
- **HTTP/WebSocket API** with REST endpoints and real-time chat.
- **Embedded Web UI** for browser-based interaction.
- **OpenClaw compatibility** for workspace files, session format, and skills system.
- **Agent tools**: bash, read_file, write_file, edit_file, memory_search, memory_get, web_fetch.
- **Session management** with persistence, compaction, search, and export.
- **Image attachment support** for multimodal LLMs.
- **Tool approval mode** for dangerous operations.
- **Zero-config startup** defaulting to `claude-cli/opus`.
- **Auto-migration** from OpenClaw config if present.

[Unreleased]: https://github.com/localgpt-app/localgpt/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/localgpt-app/localgpt/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/localgpt-app/localgpt/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/localgpt-app/localgpt/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/localgpt-app/localgpt/releases/tag/v0.1.0
