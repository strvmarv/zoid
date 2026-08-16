# zoid

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](./LICENSE-MIT)

A terminal-native AI coding agent, built in Rust. Active context management,
bring-your-own-tools via MCP, and your choice of model — Ollama, Anthropic,
OpenAI, and OpenCode Zen.

zoid manages context like a database (event-sourced conversation history with
automatic compaction and eviction), not a growing text buffer. It runs as a
single ~16 MB binary, cold-starts in milliseconds, and lets you swap models
and tools without changing your workflow.

## Install

**Linux / macOS:**
```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.sh | sh
```

**Windows (PowerShell):**
```pwsh
irm https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.ps1 | iex
```

Once installed, `zoid update` keeps your install current (anonymous,
checksum-verified).

## Quickstart

```bash
zoid
```

On first launch, zoid walks you through choosing a provider, entering your
API key, and picking a model. After that you land straight in the chat.

## Features

- **Active context management** — event-sourced conversation history with
  automatic compaction, eviction, and a token economy view. The context
  window is managed, not just filled.
- **Bring your own tools** — MCP (Model Context Protocol) support for
  connecting external tools (GitHub, databases, APIs). Built-in tools
  (read, write, edit, grep, glob, shell, subagents) included.
- **Choose your own model** — Ollama, Anthropic, OpenAI, and OpenCode Zen.
  Switch providers without restarting your session. (Google Gemini support
  is implemented but not yet user-selectable.)
- **Modes and skills** — importable skill sets (like
  [Superpowers](https://github.com/obra/superpowers)) become first-class
  modes with their own system prompts and behaviors.
- **Steerable subagents** — dispatch isolated subagents for parallel work,
  with approval gates and timeout guardrails.
- **One binary** — ~16 MB, cold-starts in milliseconds. No runtime, no
  Electron, no language server daemon.

## Building from source

```bash
cargo build --workspace --release --features zoid/local-embed
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the full development
setup, including how to run the test suite, the crate architecture, and
release conventions.

## Get involved

- [Report a bug](https://github.com/strvmarv/zoid/issues)
- [Request a feature](https://github.com/strvmarv/zoid/issues)
- [Report a vulnerability](SECURITY.md)
- [Contribute](CONTRIBUTING.md) — PRs welcome
- [Code of Conduct](CODE_OF_CONDUCT.md)

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or
[Apache License, Version 2.0](LICENSE-APACHE), at your option.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.