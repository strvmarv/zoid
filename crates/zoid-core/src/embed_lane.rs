//! The async maintenance lane's pure core: embed a batch of (id, text), append
//! to the in-memory index, and return rows to persist. Synchronous + injectable
//! so tests drive it deterministically with a FakeEmbedder (spec §5, M9). The
//! bin wraps this with store fetch (unembedded_events) + store write.

use crate::embed_index::EmbeddingIndex;
use crate::retrieval::Embedder;
use std::sync::{Arc, RwLock};
use ulid::Ulid;

pub struct EmbedLane {
    embedder: Arc<dyn Embedder>,
    index: Arc<RwLock<EmbeddingIndex>>,
}

impl EmbedLane {
    pub fn new(embedder: Arc<dyn Embedder>, index: Arc<RwLock<EmbeddingIndex>>) -> Self {
        Self { embedder, index }
    }

    /// Embed each (id, text) → unit vector, append to the index, return rows to
    /// persist. Embedding happens outside the index lock; only the append takes
    /// the write lock (briefly).
    pub fn tick(&self, batch: &[(Ulid, String)]) -> Vec<(Ulid, Vec<f32>)> {
        if batch.is_empty() {
            return Vec::new();
        }
        let texts: Vec<&str> = batch.iter().map(|(_, t)| t.as_str()).collect();
        let embs = match self.embedder.embed(&texts) {
            Ok(e) => e,
            Err(_) => return Vec::new(), // degrade: skip this batch, no panic
        };
        let mut out = Vec::with_capacity(batch.len());
        let mut idx = self.index.write().unwrap();
        for ((id, _), raw) in batch.iter().zip(embs) {
            let v = normalize(raw);
            idx.append(*id, &v);
            out.push((*id, v));
        }
        out
    }
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in &mut v {
            *x /= n;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::FakeEmbedder;

    #[test]
    fn tick_embeds_appends_and_returns_rows() {
        let emb = Arc::new(FakeEmbedder::new(16));
        let idx = Arc::new(RwLock::new(EmbeddingIndex::new(16, 100)));
        let lane = EmbedLane::new(emb, idx.clone());
        let batch = vec![
            (Ulid::from(1u128), "alpha".to_string()),
            (Ulid::from(2u128), "beta".to_string()),
        ];
        let rows = lane.tick(&batch);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, Ulid::from(1u128));
        assert_eq!(rows[0].1.len(), 16);
        assert_eq!(idx.read().unwrap().len(), 2);
    }

    #[test]
    fn tick_is_deterministic() {
        let emb = Arc::new(FakeEmbedder::new(16));
        let idx = Arc::new(RwLock::new(EmbeddingIndex::new(16, 100)));
        let lane = EmbedLane::new(emb, idx);
        let batch = vec![(Ulid::from(1u128), "same text".to_string())];
        assert_eq!(lane.tick(&batch)[0].1, lane.tick(&batch)[0].1);
    }

    struct FailingEmbedder;
    impl crate::retrieval::Embedder for FailingEmbedder {
        fn embed(&self, _texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
            anyhow::bail!("embed failed")
        }
        fn dim(&self) -> usize {
            16
        }
        fn model_id(&self) -> &str {
            "failing"
        }
    }

    #[test]
    fn tick_degrades_to_empty_on_embed_error() {
        let emb = Arc::new(FailingEmbedder);
        let idx = Arc::new(RwLock::new(EmbeddingIndex::new(16, 100)));
        let lane = EmbedLane::new(emb, idx.clone());
        let batch = vec![(Ulid::from(1u128), "alpha".to_string())];
        let rows = lane.tick(&batch);
        assert!(rows.is_empty(), "embed error must yield no rows");
        assert_eq!(
            idx.read().unwrap().len(),
            0,
            "index must be untouched on embed error"
        );
    }
}
