# ACM Slice-4 runtime spike — Phase 0 (musl link probe)

**Date:** 2026-07-08
**Question:** Which in-process runtime for the bundled local embedding + re-ranking
models links for zoid's release targets — especially `x86_64-unknown-linux-musl`?
**Method:** Minimal probe crate per path, `cargo build --release --target
x86_64-unknown-linux-musl` inside `rust:latest` Docker (+ `musl-tools`). Compile+link
only; no model download.

## Result

| Path | Deps | musl link | Detail |
|------|------|-----------|--------|
| **candle** | `candle-core/nn/transformers` 0.8, `tokenizers` 0.20 (onig) | ✅ **PASS** | Clean link. Stripped runtime binary **2.25 MB** (model weights excluded). |
| **fastembed** | `fastembed` 4.9.1 → `ort` 2.0-rc.9 | ❌ **FAIL** | (1) default `hf-hub-native-tls` → `openssl-sys` cross-compile failure. (2) with `hf-hub-rustls-tls` (past OpenSSL) → `ort-sys/build.rs:441` panics: *"downloaded binaries not available for target x86_64-unknown-linux-musl — you may have to compile ONNX Runtime from source."* |

## Decision: **candle (pure Rust)**

candle is the only runtime that ships in zoid's existing `cargo-dist` musl pipeline
unmodified. fastembed/ONNX would require building + statically linking C++ ONNX
Runtime against musl per release — a sub-project that contradicts zoid's deliberate
C-free posture (`opt-level="z"`, dropped git2 OpenSSL, `rustls-tls`). Release targets
(`dist-workspace.toml`): `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`,
`x86_64-pc-windows-msvc` — candle is pure-Rust on all three.

## Deferred to Phase 1 (fold into implementation plan)

- bge-small embedding **correctness** sanity (cosine(similar) > cosine(dissimilar)).
- **CPU latency**: embed ~50 chunks + rerank ~20 candidates, cold + warm.
- Plumbing to write: BERT forward (`candle-transformers::models::bert`) for bge-small
  embeddings + a cross-encoder head for re-ranking + `tokenizers` wiring.
