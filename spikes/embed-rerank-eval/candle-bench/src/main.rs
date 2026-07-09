//! Phase-1 candle spike: bge-small embedding + bge-reranker-base cross-encoder.
//! Correctness sanity + CPU latency (cold/warm). Throwaway benchmark harness.

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use candle_transformers::models::xlm_roberta::{
    Config as XlmConfig, XLMRobertaForSequenceClassification,
};
use hf_hub::api::sync::Api;
use std::time::Instant;
use tokenizers::Tokenizer;

const EMBED_MODEL: &str = "BAAI/bge-small-en-v1.5";
const RERANK_MODEL: &str = "BAAI/bge-reranker-base";

// ---------- Embedder ----------

struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl Embedder {
    /// Returns (embedder, model_load_ms). model_load_ms = weights -> ready
    /// (excludes download, excludes tokenizer/config parse of trivial cost).
    fn load(device: Device) -> Result<(Self, f64)> {
        let api = Api::new()?;
        let repo = api.model(EMBED_MODEL.to_string());
        let config_path = repo.get("config.json").context("dl config.json")?;
        let tokenizer_path = repo.get("tokenizer.json").context("dl tokenizer.json")?;
        let weights_path = repo.get("model.safetensors").context("dl model.safetensors")?;

        let config: BertConfig =
            serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(anyhow::Error::msg)?;

        let t = Instant::now();
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, &device)?
        };
        let model = BertModel::load(vb, &config)?;
        let load_ms = t.elapsed().as_secs_f64() * 1e3;

        Ok((
            Self {
                model,
                tokenizer,
                device,
            },
            load_ms,
        ))
    }

    /// CLS pool + L2 normalize -> 384-dim unit vector for a single text.
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(anyhow::Error::msg)?;
        let ids: Vec<u32> = enc.get_ids().to_vec();
        let type_ids: Vec<u32> = enc.get_type_ids().to_vec();
        let n = ids.len();
        let input_ids = Tensor::new(ids, &self.device)?.reshape((1, n))?;
        let token_type_ids = Tensor::new(type_ids, &self.device)?.reshape((1, n))?;
        let attn: Vec<u32> = enc.get_attention_mask().to_vec();
        let attention_mask = Tensor::new(attn, &self.device)?.reshape((1, n))?;

        // last hidden state: (1, seq, 384)
        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?;
        // CLS pooling: position 0
        let cls = hidden.i((0, 0))?; // (384,)
        let v: Vec<f32> = cls.to_vec1()?;
        Ok(l2_normalize(&v))
    }
}

// ---------- Reranker (cross-encoder) ----------

struct Reranker {
    model: XLMRobertaForSequenceClassification,
    tokenizer: Tokenizer,
    device: Device,
}

impl Reranker {
    fn load(device: Device) -> Result<(Self, f64)> {
        let api = Api::new()?;
        let repo = api.model(RERANK_MODEL.to_string());
        let config_path = repo.get("config.json").context("dl rr config.json")?;
        let tokenizer_path = repo.get("tokenizer.json").context("dl rr tokenizer.json")?;
        let weights_path = repo
            .get("model.safetensors")
            .context("dl rr model.safetensors")?;

        let config: XlmConfig =
            serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(anyhow::Error::msg)?;

        let t = Instant::now();
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)?
        };
        // bge-reranker-base = 1 relevance logit
        let model = XLMRobertaForSequenceClassification::new(1, &config, vb)?;
        let load_ms = t.elapsed().as_secs_f64() * 1e3;

        Ok((
            Self {
                model,
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

        let logits = self
            .model
            .forward(&input_ids, &attention_mask, &token_type_ids)?; // (1, 1)
        let v: Vec<f32> = logits.flatten_all()?.to_vec1()?;
        Ok(v[0])
    }
}

// ---------- helpers ----------

use candle_core::IndexOp;

fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        v.to_vec()
    } else {
        v.iter().map(|x| x / norm).collect()
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    // inputs already L2-normalized -> dot product
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn short_chunks() -> Vec<String> {
    let base = [
        "The cat sat on the mat.",
        "A dog barked at the mailman.",
        "Quarterly revenue exceeded analyst expectations.",
        "The recipe calls for two cups of flour.",
        "Rust guarantees memory safety without a garbage collector.",
        "The train departs at nine in the morning.",
        "Photosynthesis converts sunlight into chemical energy.",
        "She painted the fence a bright shade of blue.",
        "The stock market rallied on strong earnings.",
        "Mount Everest is the tallest mountain on Earth.",
    ];
    // 10 unique * 5 = 50 short sentence-length chunks
    let mut out = Vec::with_capacity(50);
    for i in 0..5 {
        for s in base.iter() {
            out.push(format!("{s} (variant {i})"));
        }
    }
    out
}

fn main() -> Result<()> {
    let device = Device::Cpu;
    println!("== candle Phase-1 bench (CPU, release) ==");
    println!("threads(rayon default): {}", rayon_threads());

    // ---------------- EMBEDDER ----------------
    println!("\n[1] Loading embedder {EMBED_MODEL} ...");
    let (embedder, embed_load_ms) = Embedder::load(device.clone())?;
    println!("    model-load (weights->ready): {embed_load_ms:.1} ms");

    // correctness sanity
    let a = embedder.embed("The cat sat on the mat.")?;
    let a2 = embedder.embed("A cat is sitting on a rug.")?;
    let b = embedder.embed("Quarterly revenue exceeded analyst expectations.")?;
    println!("    embed dim: {}", a.len());
    let cos_aa2 = cosine(&a, &a2);
    let cos_ab = cosine(&a, &b);
    let embed_pass = a.len() == 384 && cos_aa2 > cos_ab;
    println!("    cosine(A, A2 paraphrase) = {cos_aa2:.4}");
    println!("    cosine(A, B unrelated)   = {cos_ab:.4}");
    println!(
        "    EMBED CORRECTNESS: {}",
        if embed_pass { "PASS" } else { "FAIL" }
    );

    // latency: 50 short chunks, one-at-a-time (representative of per-turn preflight)
    let chunks = short_chunks();
    println!("    latency over {} chunks (per-chunk, no batching):", chunks.len());

    let t = Instant::now();
    for c in &chunks {
        let _ = embedder.embed(c)?;
    }
    let cold_ms = t.elapsed().as_secs_f64() * 1e3;

    let t = Instant::now();
    for c in &chunks {
        let _ = embedder.embed(c)?;
    }
    let warm_ms = t.elapsed().as_secs_f64() * 1e3;

    let n = chunks.len() as f64;
    println!(
        "    COLD: {:.1} ms total | {:.3} ms/chunk",
        cold_ms,
        cold_ms / n
    );
    println!(
        "    WARM: {:.1} ms total | {:.3} ms/chunk",
        warm_ms,
        warm_ms / n
    );

    // ---------------- RERANKER ----------------
    println!("\n[2] Loading reranker {RERANK_MODEL} ...");
    let rerank_result = (|| -> Result<()> {
        let (reranker, rr_load_ms) = Reranker::load(device.clone())?;
        println!("    model-load (weights->ready): {rr_load_ms:.1} ms");

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

        // sanity: top result must be a photosynthesis sentence, and beat an
        // obviously-irrelevant one (mitochondria / interest rates).
        let top = scored[0].1;
        let relevant_top = top.to_lowercase().contains("photosynthesis")
            || top.to_lowercase().contains("sunlight")
            || top.to_lowercase().contains("chlorophyll")
            || top.to_lowercase().contains("light energy");
        let worst = scored.last().unwrap().0;
        let best = scored[0].0;
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
        Ok(())
    })();

    if let Err(e) = rerank_result {
        println!("    RERANKER NOT WIRED IN SPIKE -- error: {e:#}");
    }

    println!("\n== done ==");
    Ok(())
}

fn rayon_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}
