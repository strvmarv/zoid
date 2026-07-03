# Changelog

## Unreleased

Settings redesign.
- Full-screen three-column settings (sections · fields · contextual picker) replacing the cramped card; baseline 160×40 with graceful degradation.
- Visible provider/model picker (Miller-column cascade) replacing the blind cycle; selecting a provider seeds `base_url` from the registry and jumps to model selection.
- Transport-aware provider registry: `ollama-local` / `ollama-cloud` split, `anthropic-api`, plus `[planned]` `anthropic-cli` / `anthropic-sdk` seam entries. Legacy `ollama`/`anthropic` ids alias to the new canonical ids.
- Live model discovery: the model picker fetches available models from the provider (Ollama `/api/tags`, Anthropic `/v1/models`), falling back to the registry list offline. Selecting a key-requiring provider prompts for the API key before fetching.
- `Alt+P` quick-switch overlay for changing provider + model mid-session.

## 0.1.2

Release-pipeline hardening (no functional binary changes).
- The byproduct GitHub release in the private source repo is now auto-deleted after each release (its git tag is kept); anonymous users only ever see the public releases repo with working install URLs.

## 0.1.1

Release-pipeline fix (no functional binary changes).
- Public release notes now carry the full install instructions (shell + PowerShell one-liners and the download table) with download URLs pointed at the public releases repo.
- First release to exercise the `zoid update` self-update client path (updating from 0.1.0).

## 0.1.0

First distributed release.
- Prebuilt binaries for Linux (x86_64 static musl), macOS (Apple Silicon), Windows (x86_64).
- `zoid update` — anonymous, checksum-verified self-update.
- `zoid --version` / `zoid --help`.
