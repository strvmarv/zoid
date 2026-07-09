# EmbeddingIndex vecstore bench — resume-load + brute-force scan

**Date:** 2026-07-08. Synthetic: random 384-d unit vectors in on-disk sqlite
`event_embeddings`, no ML. Single-thread, `opt-level=3`, in `rust:latest` Docker.
Measures (1) resume fill = `ORDER BY rowid DESC LIMIT cap` -> flat `Vec<f32>`,
(2) top-20 cosine (=dot, unit vecs) scan. `cap = 150_000`.

| N events | loaded (ring) | DB size | RAM | Resume LOAD | SCAN cold/warm |
|---------:|--------------:|--------:|----:|------------:|---------------:|
| 10,000   | 10,000  | 19.6 MB  | 14.6 MB  | 17.6 ms  | 2.58 / 2.58 ms |
| 50,000   | 50,000  | 97.9 MB  | 73.2 MB  | 75.4 ms  | 12.67 / 12.61 ms |
| 150,000  | 150,000 | 293.7 MB | 219.7 MB | 223.7 ms | 37.92 / 38.13 ms |
| 300,000  | 150,000 | 587.4 MB | 219.7 MB | 209.1 ms | 38.21 / 38.14 ms |

## Findings
- **Resume load is fast:** ~18 ms typical (10k), ~224 ms worst-case at the 150k cap.
  (Earlier estimate of 1–2 s was ~5–10x pessimistic.)
- **Scan:** linear in N; ~2.6 ms (10k) → ~38 ms at the 150k cap, single-threaded.
  (Earlier "sub-ms" claim was too optimistic; reality is single-digit-to-~38 ms.
  Fine off the hot path; parallelizable to ~6 ms across 6 cores if needed.)
- **Ring cap bounds everything:** the 300k row (587 MB on disk) holds RAM at 220 MB,
  load at 209 ms, scan at 38 ms — cost is bounded by `cap`, not history size.

## Design consequences
- `cap` is an informed knob: 150k ≈ 220 MB / 38 ms (generous) vs 50k ≈ 73 MB / 13 ms.
- Apply a per-crate profile override `[profile.release.package.zoid-embed] opt-level=3`
  so the scan vectorizes while the binary stays `opt-level="z"`. (Bench was -O3.)
- Brute-force in-memory is validated; no `sqlite-vec` / native dep needed at these sizes.
