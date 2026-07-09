//! Phase-1b candle spike: SMALL cross-encoder re-ranker
//! (cross-encoder/ms-marco-MiniLM-L-6-v2, ~90MB, plain BERT).
//!
//! candle-transformers 0.8 provides turnkey *ForSequenceClassification only for
//! modernbert + xlm_roberta. Plain `models::bert` exposes only the base
//! BertModel (no SeqClass head), so we HAND-WIRE the head:
//!   CLS hidden -> bert.pooler.dense + tanh (BertPooler) -> classifier Linear
//!   (out=1) -> single relevance logit.
//! Mirrors candle-bench's rerank section for correctness + CPU latency.

use anyhow::{Context, Result};
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::{Linear, Module, VarBuilder};
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use hf_hub::api::sync::Api;
use std::time::Instant;
use tokenizers::Tokenizer;

const RERANK_MODEL: &str = "cross-encoder/ms-marco-MiniLM-L-6-v2";

// ---------- Reranker (hand-wired BertForSequenceClassification) ----------

struct Reranker {
    model: BertModel,
    pooler: Linear,     // bert.pooler.dense (384 -> 384), followed by tanh
    classifier: Linear, // classifier (384 -> 1)
    tokenizer: Tokenizer,
    device: Device,
}

impl Reranker {
    /// Returns (reranker, model_load_ms). load_ms = weights -> ready.
    fn load(device: Device) -> Result<(Self, f64)> {
        let api = Api::new()?;
        let repo = api.model(RERANK_MODEL.to_string());
        let config_path = repo.get("config.json").context("dl config.json")?;
        let tokenizer_path = repo.get("tokenizer.json").context("dl tokenizer.json")?;
        let weights_path = repo
            .get("model.safetensors")
            .context("dl model.safetensors")?;

        let config: BertConfig =
            serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(anyhow::Error::msg)?;

        let t = Instant::now();
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)?
        };
        // Base encoder lives under the "bert." prefix in the safetensors.
        let model = BertModel::load(vb.pp("bert"), &config)?;
        // BertPooler: Linear(hidden, hidden) on the CLS token, then tanh.
        let pooler = candle_nn::linear(
            config.hidden_size,
            config.hidden_size,
            vb.pp("bert").pp("pooler").pp("dense"),
        )?;
        // Sequence-classification head: Linear(hidden -> 1) => single logit.
        let classifier = candle_nn::linear(config.hidden_size, 1, vb.pp("classifier"))?;
        let load_ms = t.elapsed().as_secs_f64() * 1e3;

        Ok((
            Self {
                model,
                pooler,
                classifier,
                tokenizer,
                device,
            },
            load_ms,
        ))
    }

    /// Single relevance logit for a (query, candidate) pair.
    fn score(&self, query: &str, candidate: &str) -> Result<f32> {
        let enc = self
            .tokenizer
            .encode((query, candidate), true)
            .map_err(anyhow::Error::msg)?;
        let ids: Vec<u32> = enc.get_ids().to_vec();
        let type_ids: Vec<u32> = enc.get_type_ids().to_vec();
        let attn: Vec<u32> = enc.get_attention_mask().to_vec();
        let n = ids.len();
        let input_ids = Tensor::new(ids, &self.device)?.reshape((1, n))?;
        let token_type_ids = Tensor::new(type_ids, &self.device)?.reshape((1, n))?;
        let attention_mask = Tensor::new(attn, &self.device)?.reshape((1, n))?;

        // last hidden state: (1, seq, hidden)
        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;
        // BertForSequenceClassification: pooled = tanh(pooler.dense(CLS)),
        // then logits = classifier(pooled). (dropout is a no-op at inference.)
        let cls = hidden.i((.., 0))?; // (1, hidden)
        let pooled = self.pooler.forward(&cls)?.tanh()?; // (1, hidden)
        let logits = self.classifier.forward(&pooled)?; // (1, 1)
        let v: Vec<f32> = logits.flatten_all()?.to_vec1()?;
        Ok(v[0])
    }
}

fn main() -> Result<()> {
    let device = Device::Cpu;
    println!("== candle Phase-1b MiniLM reranker bench (CPU, release) ==");
    println!("threads: {}", rayon_threads());

    println!("\nLoading reranker {RERANK_MODEL} ...");
    let (reranker, rr_load_ms) = Reranker::load(device.clone())?;
    println!("    model-load (weights->ready): {rr_load_ms:.1} ms");

    // Same query + 20 candidates as candle-bench's rerank section.
    let query = "How does photosynthesis work in plants?";
    let candidates = vec![
        "Photosynthesis is the process by which plants convert light energy into chemical energy stored in glucose.",
        "Chlorophyll in plant leaves absorbs sunlight to drive the conversion of CO2 and water into sugars.",
        "Plants use sunlight, water, and carbon dioxide to produce oxygen and energy-rich carbohydrates.",
        "The mitochondria is the powerhouse of the cell.",
        "Interest rates were raised by the central bank last quarter.",
        "The quick brown fox jumps over the lazy dog.",
        "A balanced diet includes proteins, fats, and carbohydrates.",
        "The Eiffel Tower is located in Paris, France.",
        "Water boils at 100 degrees Celsius at sea level.",
        "Basketball is played with two teams of five players.",
        "Leaves contain chloroplasts where the light-dependent reactions occur.",
        "The stock exchange closed higher on Friday afternoon.",
        "Rust is a systems programming language focused on safety.",
        "Cats are known for their independent behavior.",
        "The recipe requires preheating the oven to 350 degrees.",
        "Sunlight is captured by pigments and used to split water molecules.",
        "The novel was published in the early twentieth century.",
        "Electric cars run on rechargeable battery packs.",
        "The football match ended in a scoreless draw.",
        "Carbon dioxide enters the leaf through tiny pores called stomata.",
    ];

    // score all 20 (cold)
    let t = Instant::now();
    let mut scored: Vec<(f32, &str)> = Vec::new();
    for c in &candidates {
        scored.push((reranker.score(query, c)?, *c));
    }
    let rr_cold_ms = t.elapsed().as_secs_f64() * 1e3;

    // warm
    let t = Instant::now();
    for c in &candidates {
        let _ = reranker.score(query, c)?;
    }
    let rr_warm_ms = t.elapsed().as_secs_f64() * 1e3;

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    println!("    ranked (query = {:?}):", query);
    for (i, (s, c)) in scored.iter().enumerate() {
        let short = if c.len() > 62 { &c[..62] } else { c };
        println!("      #{:<2} logit={:>8.3}  {}", i + 1, s, short);
    }

    let top = scored[0].1.to_lowercase();
    let relevant_top = top.contains("photosynthesis")
        || top.contains("sunlight")
        || top.contains("chlorophyll")
        || top.contains("light energy");
    let best = scored[0].0;
    let worst = scored.last().unwrap().0;
    let rr_pass = relevant_top && best > worst;
    println!(
        "    RERANK CORRECTNESS: {}  (top relevant={}, best {:.3} > worst {:.3})",
        if rr_pass { "PASS" } else { "FAIL" },
        relevant_top,
        best,
        worst
    );

    let m = candidates.len() as f64;
    println!(
        "    COLD: {:.1} ms total | {:.3} ms/pair ({} pairs)",
        rr_cold_ms,
        rr_cold_ms / m,
        candidates.len()
    );
    println!(
        "    WARM: {:.1} ms total | {:.3} ms/pair",
        rr_warm_ms,
        rr_warm_ms / m
    );

    println!("\n== done ==");
    Ok(())
}

fn rayon_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}
