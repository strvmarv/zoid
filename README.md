# zoid

A terminal-native AI coding agent, built in Rust. Active context management,
bring-your-own-tools via MCP, and your choice of model — Ollama, Anthropic,
OpenAI, Google Gemini, and OpenCode Zen.

zoid is early and evolving. Issues, discussion, and pull requests are welcome
— see [CONTRIBUTING.md](CONTRIBUTING.md).

## Install

Download the latest release for your platform and run the installer:

- **Linux / macOS:** `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.sh | sh`
- **Windows (PowerShell):** `irm https://github.com/strvmarv/zoid/releases/latest/download/zoid-installer.ps1 | iex`

Once installed, `zoid update` keeps your install current.

## Quickstart

```bash
zoid
```

On first launch with no provider configured, zoid walks you through choosing
a provider, entering your API key, and (where applicable) picking a model.
After that you land straight in the chat.

## Building from source

```bash
cargo build --workspace --release
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development setup,
including how to run the test suite.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or
[Apache License, Version 2.0](LICENSE-APACHE), at your option.
