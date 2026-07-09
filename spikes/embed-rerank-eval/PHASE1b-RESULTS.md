# Phase-1b Results — Small cross-encoder re-ranker on candle

**Goal:** validate a SMALL (~90MB) cross-encoder re-ranker to replace the 1.1GB
`bge-reranker-base` in the candle (pure-Rust) runtime path.

## Path validated

**PRIMARY path (hand-wired) — SUCCESS.** Model
`cross-encoder/ms-marco-MiniLM-L-6-v2` (plain BERT, 6 layers, hidden 384).

candle-transformers 0.8.4 has no `BertForSequenceClassification` (turnkey
SeqClass exists only for `modernbert` + `xlm_roberta`), so the classification
head was hand-wired on candle's base `BertModel`:

- Base encoder loaded from the `bert.` prefix (`BertModel::load(vb.pp("bert"), &cfg)`).
- `BertPooler`: `bert.pooler.dense` Linear (384→384) on the CLS token, then `tanh`.
- Head: `classifier` Linear (384→1) → single relevance logit.
- Pair tokenized with the model's `tokenizer.json` (query, candidate) so
  token_type_ids are set.

Worked on the **first build iteration** — no weight-name mismatch, no need for
the modernbert fallback. Safetensors keys confirmed up front:
`bert.pooler.dense.{weight,bias}`, `classifier.{weight,bias}` (shape `[1,384]`).
Config declares activation Identity, so the classifier emits a raw logit.

## Environment

CPU (`Device::Cpu`), `--release`, `rust:latest` Docker, 6 threads. candle-core /
candle-nn / candle-transformers 0.8.4, tokenizers 0.20.

## 1. Correctness — **PASS**

Same query + 20 candidates as the existing `candle-bench` rerank section.
Query: `"How does photosynthesis work in plants?"`

```
#1  logit=   8.443  Photosynthesis is the process by which plants convert light en...
#2  logit=   0.773  Chlorophyll in plant leaves absorbs sunlight to drive the conv...
#3  logit=  -0.913  Plants use sunlight, water, and carbon dioxide to produce oxyg...
#4  logit=  -4.066  Sunlight is captured by pigments and used to split water molec...
#5  logit=  -5.429  Carbon dioxide enters the leaf through tiny pores called stoma...
#6  logit=  -6.366  Leaves contain chloroplasts where the light-dependent reaction...
#7  logit= -11.089  The mitochondria is the powerhouse of the cell.
...
#20 logit= -11.295  Electric cars run on rechargeable battery packs.
```

Top result is a photosynthesis sentence; best (8.443) >> worst (-11.295). The
six on-topic candidates occupy the top six slots, cleanly separated (~-11 floor)
from all off-topic ones. **PASS.**

## 2. Latency (CPU, 20 pairs, per-pair, no batching)

| Metric              | Value                  |
|---------------------|------------------------|
| Model-load (weights→ready) | **51.5 ms**     |
| Rerank COLD (20 pairs)     | 411.5 ms total / **20.58 ms/pair** |
| Rerank WARM (20 pairs)     | 415.5 ms total / **20.78 ms/pair** |

Cold and warm are effectively identical (no warmup penalty on CPU).

## 3. On-disk size

`model.safetensors` = **90,870,598 bytes ≈ 86.7 MiB (90.9 MB)** — vs
bge-reranker-base at ~1.1 GB (~12x smaller).

## Verdict

**Yes — good enough to replace `bge-reranker-base` in v1.** It runs turnkey-free
on candle with a ~15-line hand-wired head, is ~12x smaller (91MB vs 1.1GB),
loads in ~50ms, ranks the relevant docs correctly with strong score separation,
and costs ~21 ms/pair on CPU (single-threaded per-pair, batchable to improve).
