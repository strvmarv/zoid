# ACM Slice-4 runtime spike — Phase 1 (candle correctness + CPU latency)

**Date:** 2026-07-08
**Runtime:** candle (decided in Phase 0). CPU, 6 threads, `--release`, in `rust:latest` Docker.
**Harness:** `candle-bench/` — bge-small embedder (CLS pool + L2 norm) and
bge-reranker-base cross-encoder (`XLMRobertaForSequenceClassification`, 1 logit).

## Results

| Metric | Embedder `bge-small-en-v1.5` | Reranker `bge-reranker-base` |
|--------|------------------------------|------------------------------|
| Correctness | ✅ PASS — cos 0.8056 (paraphrase) vs 0.3697 (unrelated); dim 384 | ✅ PASS — relevant +11.98…+5.19, irrelevant −0.72…−15.56 |
| Model load (weights→ready) | 64 ms | 524 ms |
| Latency (single-item, no batching) | **30.5–30.9 ms/chunk** (50 chunks) | **~100 ms/pair** (20 pairs ≈ 2.0 s) |
| On-disk weights | ~130 MB | **~1.1 GB** |

Embed correctness output: cosine(paraphrase)=0.8056 > cosine(unrelated)=0.3697.
Rerank ordering: all 5 photosynthesis sentences ranked above all noise; clean margin.

## Interpretation for the design

- **Embedder = always-on core.** 130 MB, ~30 ms/chunk, and per the ACM design runs
  on the **async maintenance lane** (not the synchronous pre-flight gate). Off the
  hot path this is a non-issue; batching would reduce it further. Clear v1 inclusion.
- **Reranker = heavy, asymmetric tenant.** 1.1 GB weights (~8.5× the embedder) and
  ~100 ms/pair → reranking 20 candidates ≈ 2 s. Quality is excellent, but the
  size+latency argue it should be **optional / lazily fetched / or swapped for a
  much smaller MiniLM-class cross-encoder (~80–90 MB)** — or the cross-encoder is
  **deferred in v1** in favor of hybrid FTS + vector (bi-encoder cosine) ordering.

## Notes
- Numbers are native-gnu in Docker; musl perf is comparable for pure-Rust gemm.
- No batching used; per-item is the pessimistic case. Real embed lane can batch.
- `PHASE1-run.log` tee failed (host path) but the run itself (stdout) succeeded;
  captured numbers are from the run.
