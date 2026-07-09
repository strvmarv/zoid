//! candle bge-small embedder. CLS pooling + L2 normalize → 384-d unit vectors.
//! Lifted from the validated spike (spikes/embed-rerank-eval/candle-bench).

pub mod fetch;

use anyhow::{Context, Result};
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
use std::path::Path;
use tokenizers::Tokenizer;
use zoid_core::retrieval::Embedder;

pub struct CandleEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl CandleEmbedder {
    pub fn load(cache_dir: &Path, auto_download: bool) -> Result<Self> {
        let w = fetch::ensure_weights(cache_dir, auto_download)?;
        let cfg: BertConfig = serde_json::from_str(&std::fs::read_to_string(&w.config)?)?;
        let tokenizer = Tokenizer::from_file(&w.tokenizer).map_err(anyhow::Error::msg)?;
        let device = Device::Cpu;
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[w.weights], DTYPE, &device)? };
        let model = BertModel::load(vb, &cfg).context("load bert")?;
        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(anyhow::Error::msg)?;
        // Copy to owned Vec<u32> before Tensor::new (matches the validated spike,
        // candle-bench/src/main.rs:63-66).
        let ids_v: Vec<u32> = enc.get_ids().to_vec();
        let types_v: Vec<u32> = enc.get_type_ids().to_vec();
        let mask_v: Vec<u32> = enc.get_attention_mask().to_vec();
        let n = ids_v.len();
        let ids = Tensor::new(ids_v, &self.device)?.reshape((1, n))?;
        let types = Tensor::new(types_v, &self.device)?.reshape((1, n))?;
        let mask = Tensor::new(mask_v, &self.device)?.reshape((1, n))?;
        let hidden = self.model.forward(&ids, &types, Some(&mask))?;
        let cls: Vec<f32> = hidden.i((0, 0))?.to_vec1()?;
        let norm: f32 = cls.iter().map(|x| x * x).sum::<f32>().sqrt();
        Ok(if norm > 0.0 {
            cls.iter().map(|x| x / norm).collect()
        } else {
            cls
        })
    }
}

impl Embedder for CandleEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_one(t)).collect()
    }
    fn dim(&self) -> usize {
        384
    }
    fn model_id(&self) -> &str {
        "bge-small-en-v1.5"
    }
}

#[cfg(test)]
mod smoke {
    use super::*;
    #[test]
    #[ignore = "downloads ~130MB bge-small; run manually"]
    fn embeds_384_and_paraphrase_beats_unrelated() {
        let dir = tempfile::tempdir().unwrap();
        let e = CandleEmbedder::load(dir.path(), true).unwrap();
        let v = e.embed(&["the cat sat on the mat"]).unwrap();
        assert_eq!(v[0].len(), 384);
        let a = &e.embed(&["the cat sat on the mat"]).unwrap()[0];
        let a2 = &e.embed(&["a cat is sitting on a rug"]).unwrap()[0];
        let b = &e.embed(&["quarterly revenue beat expectations"]).unwrap()[0];
        let cos = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
        assert!(cos(a, a2) > cos(a, b));
    }
}
