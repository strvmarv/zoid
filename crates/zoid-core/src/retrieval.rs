//! Retrieval & relevance seams (spec §3.7). Pure trait declarations, threaded as
//! `Option<Arc<dyn …>>` by consumers (None in Slices 0–2). Slice 4 supplies
//! in-process implementations — no rearchitecture, only lit-up seams.

use ulid::Ulid;

use crate::embed_index::EmbeddingIndex;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, PartialEq)]
pub struct RecallCandidate {
    pub event_id: Ulid,
    pub content: String,
    pub lexical_score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub candidate: RecallCandidate,
    pub score: f32,
}

/// In-process embedding model (candidate impls: fastembed/ONNX bge-small, candle).
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
    fn dim(&self) -> usize;
    fn model_id(&self) -> &str;
}

/// In-process cross-encoder that refines candidate ordering for precision.
pub trait Reranker: Send + Sync {
    fn rerank(&self, query: &str, candidates: &[RecallCandidate]) -> Vec<Scored>;
}

/// One stage of the staged recall pipeline (Slice 2 = `[Fts5Source]`;
/// Slice 4 adds a `VectorSource`).
pub trait CandidateSource: Send + Sync {
    fn candidates(&self, query: &str, k: usize) -> Vec<RecallCandidate>;
}

fn l2_normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 { for x in &mut v { *x /= n; } }
    v
}

/// Deterministic test/dev embedder: hashes tokens into a fixed-dim unit vector.
/// Not semantic — only for tests and the candle-free build path.
pub struct FakeEmbedder { dim: usize }
impl FakeEmbedder { pub fn new(dim: usize) -> Self { Self { dim } } }
impl Embedder for FakeEmbedder {
    fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| {
            let mut v = vec![0f32; self.dim];
            for tok in t.split_whitespace() {
                let mut h: u64 = 1469598103934665603;
                for b in tok.bytes() { h ^= b as u64; h = h.wrapping_mul(1099511628211); }
                v[(h as usize) % self.dim] += 1.0;
            }
            l2_normalize(v)
        }).collect())
    }
    fn dim(&self) -> usize { self.dim }
    fn model_id(&self) -> &str { "fake" }
}

/// Semantic recall stage: embeds the query and scans the in-memory index.
pub struct VectorSource {
    embedder: Arc<dyn Embedder>,
    index: Arc<RwLock<EmbeddingIndex>>,
}
impl VectorSource {
    pub fn new(embedder: Arc<dyn Embedder>, index: Arc<RwLock<EmbeddingIndex>>) -> Self {
        Self { embedder, index }
    }
    /// Embed one text to a unit vector (reused by the lane to fill the index).
    pub fn embed_unit(&self, text: &str) -> Option<Vec<f32>> {
        self.embedder.embed(&[text]).ok().and_then(|mut v| v.pop()).map(l2_normalize)
    }
}
impl CandidateSource for VectorSource {
    fn candidates(&self, query: &str, k: usize) -> Vec<RecallCandidate> {
        let Some(q) = self.embed_unit(query) else { return Vec::new() };
        let idx = self.index.read().unwrap();
        idx.scan_topk(&q, k).into_iter().map(|(event_id, score)| RecallCandidate {
            event_id, content: String::new(), lexical_score: score,
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // A trivial impl proves the seams are object-safe and usable as trait objects.
    struct NoopReranker;
    impl Reranker for NoopReranker {
        fn rerank(&self, _q: &str, cands: &[RecallCandidate]) -> Vec<Scored> {
            cands
                .iter()
                .map(|c| Scored {
                    candidate: c.clone(),
                    score: c.lexical_score,
                })
                .collect()
        }
    }
    #[test]
    fn seams_are_object_safe() {
        let r: Box<dyn Reranker> = Box::new(NoopReranker);
        let c = RecallCandidate {
            event_id: Ulid::from(1u128),
            content: "x".into(),
            lexical_score: 1.0,
        };
        assert_eq!(r.rerank("q", &[c]).len(), 1);
    }
}

#[cfg(test)]
mod vector_tests {
    use super::*;
    use crate::embed_index::EmbeddingIndex;
    use std::sync::{Arc, RwLock};

    #[test]
    fn vectorsource_finds_seeded_event() {
        let emb = Arc::new(FakeEmbedder::new(16));
        let idx = Arc::new(RwLock::new(EmbeddingIndex::new(16, 100)));
        // seed the index with the embedding of a known text
        let v = emb.embed(&["alpha beta"]).unwrap().remove(0);
        idx.write().unwrap().append(Ulid::from(7u128), &v);
        idx.write().unwrap().append(
            Ulid::from(8u128),
            &emb.embed(&["totally different"]).unwrap().remove(0),
        );
        let vs = VectorSource::new(emb, idx);
        let cands = vs.candidates("alpha beta", 1);
        assert_eq!(cands.len(), 1);
        assert_eq!(u128::from(cands[0].event_id), 7);
    }
}
