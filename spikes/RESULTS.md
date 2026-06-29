# zoid language spike — results

Decision experiment: Rust vs .NET 10 for zoid, scored on the design's riskiest axes.
Dev box: Rust 1.96, .NET 10.0.201, clang/gcc, Arch Linux.

## Rust (ratatui + crossterm + tokio + wasmtime + tree-sitter)

| Axis | Result |
|---|---|
| **R1 TUI** | Compiles clean. Semantic-zoom toggle, drawer+Tab focus, **mouse hit-testing/select**, async streaming via `tokio::select!` over `EventStream` + mpsc — all hand-written. ~330 LOC. Immediate-mode: I render whatever projection I want each frame (zoom = just choosing which lines to build). Floor cost: focus + mouse hit-testing are **hand-rolled** (no built-in focus system). |
| **R2 wasm** | ✅ `wasmtime`: load + call sandboxed `add(20,22)=42`. ~12 lines, trivial. |
| **R2 tree-sitter** | ✅ found `fn verify` byte-range 0..58 (the "select a symbol" primitive). ~20 lines. |
| **R3 binary size** | **6.2 MB** release, single static-ish binary — *including a full WASM JIT engine*. |
| **R3 cold start** | ~**0.00s** (sub-10ms), RSS ~3.8 MB. |
| **Build time** | debug cold ~60s; **release (LTO) 2m11s** — wasmtime dominates. Incremental debug rebuild **2.1s**. |
| **Friction** | One type error + one lifetime warning. Borrow checker was a non-issue at this size; the async+TUI wiring (`select!`) is idiomatic and clean. |

Notes: ceiling claims (custom render, wasm, tree-sitter) all confirmed with low ceremony. The real Rust cost is the *floor* (hand-built focus/mouse) and **release build time**.

## .NET 10 (Terminal.Gui v2 + Wasmtime + tree-sitter + NativeAOT)

| Axis | Result |
|---|---|
| **R1 TUI** | Builds clean. `ListView` gives **selection + mouse for free**; **Tab focus is automatic** across focusable views; `FrameView` gives titled borders; config/themes built in. Zoom = swap `ListView` source; streaming via `Application.Invoke`. ~150 LOC — **less hand-rolling than ratatui** (didn't build focus or mouse hit-testing myself). Floor is higher. |
| **R1 caveat** | Terminal.Gui v2's static `Application` API is marked **`[Obsolete]`** ("legacy static Application object is going away") — v2 is **mid-migration**; API churn risk for a long-lived product. |
| **R2 wasm (JIT)** | ✅ `Wasmtime` add(20,22)=42. |
| **R2 wasm (NativeAOT)** | ✅ **runs under AOT** (returns 42, exit 0) despite IL3053/IL2104 trim/AOT *warnings* from `Wasmtime.Dotnet`. **Falsifies "WASM is Rust-only."** Cost: `libwasmtime.so` is a **separate 25 MB native sidecar**, not statically linked. |
| **R2 tree-sitter** | Mechanism confirmed: `TreeSitter.DotNet` ships cross-platform native grammars (incl. linux-x64, 112 `.so`), loads via `NativeLibrary` P/Invoke (same path Wasmtime proved under AOT). **Binding ergonomics rougher** than Rust's crate; end-to-end parse not run on .NET (run on Rust). |
| **R3 binary** | NativeAOT exe **20 MB** + `libwasmtime.so` 25 MB + `libonigwrap.so` 0.5 MB → **multi-file (~45 MB)**. (Even without wasm, Terminal.Gui pulls native `oniguruma` → still multi-file.) |
| **R3 cold start** | ~**0.06s** (60 ms), RSS ~28 MB. |
| **Build time** | JIT build seconds; **NativeAOT publish 26s** — far faster than Rust's 2m11s LTO. |
| **Friction** | Only namespace/API discovery (v2 reorg: `Terminal.Gui.App` / `.Views` / `.ViewBase` / `.Input` / `.Drawing`). No borrow-checker; fast iteration. |

## Verdict

The spike **falsified the two biggest pro-Rust ceiling claims**: .NET hosts **WASM plugins under NativeAOT** and has **cross-platform tree-sitter**. So those are no longer reasons to switch — they're reachable in both.

What genuinely survives for **Rust**:
- **Leaner, single-file distribution** (6.2 MB one file vs .NET's multi-file ~20–45 MB with native sidecars).
- **Faster cold start / lower RSS** (~10 ms/4 MB vs ~60 ms/28 MB) — but minor for a long-running TUI you keep open.
- **More natural substrate for fully-custom rendering** (post-v1 canvas ②) and **more stable TUI lib** (ratatui vs Terminal.Gui v2's in-flight API migration).

What genuinely favors **.NET**:
- **Higher TUI floor** → faster v1 (ListView selection+mouse, auto Tab focus, FrameView, config all free; ratatui made me hand-roll focus + mouse).
- **Much faster builds** (AOT 26s vs 2m11s) + **user's daily fluency** → compounding velocity & maintainability.
- Capability parity on the things we feared losing (WASM, tree-sitter).

**Recommendation: stay with .NET 10** — now evidence-based, not default. The Rust temptations turned out reachable in .NET; the price (multi-file native distribution, slower cold start) is acceptable for a long-running TUI. Choose Rust **only if** "leanest single binary + best-in-class bespoke rendering" becomes the project's defining priority over velocity and your fluency.

**Spec impact:** add a distribution note — NativeAOT deployment is **multi-file** (native sidecars: `libwasmtime.so` if WASM plugins ship, `libonigwrap.so` from Terminal.Gui), packaged as an archive/installer, not a bare single file.

