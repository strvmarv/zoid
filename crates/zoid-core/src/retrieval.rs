//! Retrieval & relevance seams (spec §3.7). Pure trait declarations, threaded as
//! `Option<Arc<dyn …>>` by consumers (None in Slices 0–2). Slice 4 supplies
//! in-process implementations — no rearchitecture, only lit-up seams.

use ulid::Ulid;

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
