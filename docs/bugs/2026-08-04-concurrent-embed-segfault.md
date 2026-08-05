# Bug: Concurrent local-embed instances segfault (candle-core mmap)

### Status

Open — root cause narrowed to candle-core's `unsafe` mmap/slice path; not yet
reproduced in isolation.

### Symptom

Two instances of `zoid` built with the `local-embed` feature segfaulted
simultaneously while streaming a model response. The TUI disappeared and the
shell printed a "segfault" message. Both instances died mid-turn — the event
log shows their sessions ending on `ModelDelta` text fragments with no
`TurnComplete` event, the signature of a process killed while the provider
stream was active.

Reported by the user as: "we just had two simultaneous instances of zoid
segfault" and "the message on the screen said segfault."

### Reproduction (suspected)

```bash
# Terminal 1
cargo run --release --features zoid/local-embed -p zoid -- --yolo
# Terminal 2 (concurrent)
cargo run --release --features zoid/local-embed -p zoid -- --yolo
```

Both instances load the BGE-small embedding model (`bge-small-en-v1.5`) via
candle-core's `unsafe { VarBuilder::from_mmaped_safetensors(...) }`, spawn a
CPU-bound embed maintenance thread, and begin embedding events. The crash
occurs after both are running.

**Not yet confirmed:** whether a single local-embed instance crashes, or only
two running concurrently. The currently-running instances (rebuilt without
the feature) do not crash, which establishes the feature as the trigger but
doesn't isolate the concurrency factor.

### Evidence

**The crash instances were built with `local-embed`:**

- fish history (`~/.local/share/fish/fish_history`) shows
  `cargo run --release --features zoid/local-embed -p zoid -- --yolo` at
  21:36:30 and 21:51:56 CDT, immediately before the 22:27 restart.
- The currently-running instances (`target/release/zoid`, built 21:35:51
  *without* the feature) have **0 candle symbols** (`nm | grep -ci candle`)
  and have not crashed.
- `local-embed` is the only `unsafe`/native-FFI surface in zoid's dependency
  tree. `grep -rn "unsafe" crates/` finds exactly one real `unsafe` block:
  `zoid-embed/src/lib.rs:37` —
  `unsafe { VarBuilder::from_mmaped_safetensors(&[w.weights], DTYPE, &device)? }`.
- The candle-core safetensors loader (`candle-core-0.8.4/src/safetensors.rs`)
  uses `memmap2::MmapOptions` (line 282) to mmap the 133MB safetensors file,
  then does raw pointer slicing via `std::slice::from_raw_parts` (lines 105,
  135) on the mmap'd data.

**Embed is enabled by default when compiled in:**

- `crates/zoid-core/src/config.rs:124` — "Master switch (default true when
  compiled in with feature `local-embed`)."
- `~/.config/zoid/config.toml` has no `embed.enabled = false` line, so both
  crash instances loaded candle, mmap'd the safetensors, and spawned the
  embed maintenance thread (`main.rs:2604`, `std::thread::spawn`).

**Both instances shared the same resources:**

- `fuser ~/.local/share/zoid/zoid.db` — both PIDs hold the same SQLite
  database open (WAL mode, `busy_timeout = 5000ms`, bundled SQLite with FTS5).
- `~/.cache/zoid/models/bge-small-en-v1.5/model.safetensors` — both instances
  mmap the same 133MB safetensors file (dated 2026-07-09).
- The embed maintenance thread (`main.rs:2599-2630`) runs on a blocking OS
  thread, polling `session.unembedded_events_all` and running candle's CPU
  tensor ops. Two instances = two such threads racing on the same mmap'd
  weights and the same `zoid.db`.

**Sessions ended mid-stream:**

- Session `01KZ7D1S...` ("give me a summary of this repository") — last event
  22:07:45, ends on `ModelDelta("dist")` + `Usage`, no `TurnComplete`.
- Session `01KZ7DA32...` ("give me a summary of this repository") — last event
  22:38:58, ends on `ModelDelta("side‑by‑side")` + `Usage`, no `TurnComplete`.
- Both were actively generating when the process died.

**Kernel/core evidence is absent (the gap):**

- `journalctl -k` (persistent kernel log, ~1190 lines/hour, no gaps) shows
  **only** `dotnet` (the `bkith-api-1` Docker container restart-loop, UID 999)
  trapping today. No `traps:` entry for any `zoid` or `target/` process.
- `coredumpctl list` — only `dotnet` cores; no zoid cores retained or rotated
  out (`grep -i "Removed old coredump" | grep -v dotnet` is empty).
- `dmesg` is permission-blocked (`kernel.dmesg_restrict = 1`), so the
  ring-buffer could not be directly inspected — but `journalctl -k` reads the
  same kernel source persistently and shows no zoid trap.

  A real SIGSEGV should appear in `journalctl -k` as a `traps:` line. Its
  absence means either (a) the crash was a `SIGABRT` from a candle C
  dependency (e.g. a BLAS/OpenMP assertion) rather than a raw SIGSEGV — the
  shell still prints "Segmentation fault" for some abort paths; (b) journald
  rate-limited or flushed the entry under the dotnet crash-loop load (no
  explicit suppression message was found, but the ~60s crash cadence is
  heavy); or (c) the embed thread crashed and the process exited via a path
  that didn't produce a kernel trap (e.g. `std::process::exit` from a signal
  handler in a native library).

### Root cause hypothesis

**candle-core 0.8.4's `unsafe` mmap + `from_raw_parts` path, exercised by two
concurrent local-embed instances.** The `VarBuilder::from_mmaped_safetensors`
call (the sole `unsafe` block in zoid's own code) mmap's the safetensors file
via `memmap2` and the candle CPU backend then slices the mmap'd region with
`std::slice::from_raw_parts` (candle-core `safetensors.rs:105,135`). Two
instances both mmap'ing the same file and running CPU tensor ops through
this `unsafe` code is the most likely segfault surface. A corrupted or
truncated safetensors file would make this deterministic even with one
instance.

The SQLite WAL contention (`busy_timeout = 5000ms`) is ruled out as the
crash cause: a `SQLITE_BUSY` error returns an `Err` to Rust, not a signal.
Two instances contending on the writer would surface as a turn error in the
TUI, not a segfault.

### Alternative hypotheses (ruled out or unlikely)

- **Pure-Rust panic** — ruled out. `Cargo.toml:48` sets `panic = "abort"`,
  so a panic → `SIGABRT` → kernel logs `traps:` + coredumpctl captures a
  core. Neither is present.
- **`std::process::exit`** — ruled out. The five `process::exit` sites
  (`expiry.rs:46,50`, `main.rs:2282,2371,2443`) are all startup error paths
  (bad resume id, build expired, ambiguous session), not runtime. The
  sessions were mid-stream when they died, not at startup.
- **ollama backend failure** — unlikely. `ollama.service` is alive and
  serving (last inference 19:39, no errors in its journal). A provider
  error would return an `Err` to `run_turn_inner`, surfaced as a turn error
  in the TUI, not a segfault.
- **The dotnet crash-loop** — unrelated. Every `dotnet` segfault is the
  `bkith-api-1` Docker container (`dotnet Bkith.Api.dll --urls
  http://0.0.0.0:5000`), UID 999, restarting on a ~60s loop. It shares no
  resources with zoid.
- **`panic = "abort"` + panic hook swallowing the trace** — the hook
  (`obs.rs:120`) only logs to tracing then chains to the previous (default)
  hook; it does not call `process::exit`. A panic would still abort+core.

### Suggested investigation steps

1. **Reproduce in isolation.** Run one `local-embed` instance (not two) and
   exercise it (send a message that triggers embedding). If it crashes solo,
   the concurrency factor is irrelevant and the bug is in candle's
   mmap/slice code or a corrupted safetensors file. If only paired crashes,
   it's a shared-resource issue (mmap or SQLite).
2. **Verify safetensors integrity.**
   `python3 -c "from safetensors import safe_open; f=safe_open('$HOME/.cache/zoid/models/bge-small-en-v1.5/model.safetensors','rt'); print(len(f.keys()))"`.
   A truncated/corrupted file mmap'd and sliced via `from_raw_parts` would
   segfault deterministically.
3. **Disable embed to confirm.** Set `embed.enabled = false` in
   `~/.config/zoid/config.toml` and run two `local-embed` instances. If they
   don't crash, embed is definitively the trigger.
4. **Upgrade candle.** `candle-core = "0.8"` (`crates/zoid-embed/Cargo.toml`)
   is old; the 0.8.4 release is over a year old. The mmap and CPU backend
   have had fixes in later versions. Try bumping to the latest 0.9.x and
   re-running.
5. **Capture the crash live.** Run under `rust-lldb` or with
   `RUST_BACKTRACE=full` and `ZOID_LOG=/tmp/zoid-crash.jsonl` so the tracing
   panic hook (`obs.rs:120`) writes the panic location to the JSON log
   before abort. A `SIGABRT` from a native library may not produce a Rust
   backtrace, but the tracing hook fires before abort and may record the
   thread/context.
6. **Check the kernel log without the dotnet filter.** If a reproduction
   produces a `traps:` line for `target/release/zoid`, the earlier absence
   was journald rate-limiting under the dotnet crash-loop load. If still
   absent, the crash is a native `abort()` (SIGABRT), not a raw SIGSEGV.

### Workaround

Do not run two `local-embed`-enabled zoid instances concurrently. If local
embed is not needed, build without the feature (`cargo run --release -p zoid`,
no `--features zoid/local-embed`) — the running instances confirm this is
stable. If embed is needed, set `embed.enabled = false` in config.toml until
the root cause is confirmed.

### Files of interest

- `crates/zoid-embed/src/lib.rs:37` — the `unsafe` mmap call (zoid's only
  `unsafe` block).
- `crates/zoid/src/main.rs:2559-2630` — the `local-embed` feature gate and
  the embed maintenance thread spawn.
- `crates/zoid-core/src/config.rs:124` — `embed.enabled` defaults to true
  when compiled in.
- `crates/zoid-embed/Cargo.toml:8-10` — `candle-core = "0.8"`,
  `candle-nn = "0.8"`, `candle-transformers = "0.8"`.
- `~/.cache/zoid/models/bge-small-en-v1.5/model.safetensors` — the 133MB
  mmap'd weights file.
- `~/.local/share/zoid/zoid.db` — shared SQLite database (WAL mode,
  `busy_timeout = 5000ms`), held open by both instances.

### Timeline (2026-08-04, CDT)

- 21:35:51 — `target/release/zoid` rebuilt **without** `local-embed`
  (0 candle symbols). This is the binary the currently-running instances
  use; they do not crash.
- 21:36:30 — `cargo run --release --features zoid/local-embed -p zoid -- --yolo`
  (fish history ts 1785897390). This rebuilds **with** candle and launches
  instance 1.
- 21:51:56 — `cargo run --release --features zoid/local-embed -p zoid`
  (fish history ts 1785898316). Launches instance 2.
- 22:07:45 — session `01KZ7D1S...` ends mid-stream (last event:
  `ModelDelta("dist")`, no `TurnComplete`).
- 22:27:16 / 22:27:41 — the two currently-running instances (no feature)
  start. The crashed instances are gone by this point.
- 22:38:58 — session `01KZ7DA32...` ends mid-stream (last event:
  `ModelDelta("side‑by‑side")`, no `TurnComplete`).

### Related

- `docs/bugs/subagent-dispatch-language-drift.md` — unrelated bug, same
  investigation session.