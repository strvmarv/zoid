//! In-memory FIFO ring of L2-normalized embedding vectors + brute-force cosine
//! (= dot, since unit vectors) top-K scan. Pure; no candle. Bounded by `cap`:
//! resume-load, RAM, and scan cost are all O(cap), never O(history)
//! (spikes/embed-rerank-eval/VECSTORE-RESULTS.md).

use ulid::Ulid;

pub struct EmbeddingIndex {
    // Intentionally NO model_id field: staleness is handled upstream by only
    // loading/writing the active model's rows (store methods filter by model_id).
    dim: usize,
    cap: usize,
    vectors: Vec<f32>, // len == len_rows*dim, row r at [r*dim..(r+1)*dim]
    ids: Vec<Ulid>,
    write: usize, // next slot to overwrite once full
    len: usize,
}

impl EmbeddingIndex {
    pub fn new(dim: usize, cap: usize) -> Self {
        let cap = cap.max(1);
        Self { dim, cap, vectors: Vec::with_capacity(cap * dim), ids: Vec::with_capacity(cap), write: 0, len: 0 }
    }

    pub fn dim(&self) -> usize { self.dim }
    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Append (overwriting the oldest slot once full). `vec.len()` must == dim;
    /// mismatched vectors are ignored (defensive).
    pub fn append(&mut self, id: Ulid, vec: &[f32]) {
        if vec.len() != self.dim { return; }
        if self.len < self.cap {
            self.vectors.extend_from_slice(vec);
            self.ids.push(id);
            self.len += 1;
        } else {
            let off = self.write * self.dim;
            self.vectors[off..off + self.dim].copy_from_slice(vec);
            self.ids[self.write] = id;
        }
        self.write = (self.write + 1) % self.cap;
    }

    /// Top-K by dot product (== cosine for unit vectors). Query need not be
    /// pre-truncated; only the first `dim` values are used.
    pub fn scan_topk(&self, query: &[f32], k: usize) -> Vec<(Ulid, f32)> {
        if query.len() < self.dim || k == 0 { return Vec::new(); }
        let q = &query[..self.dim];
        let mut top: Vec<(Ulid, f32)> = Vec::with_capacity(k + 1);
        for r in 0..self.len {
            let row = &self.vectors[r * self.dim..(r + 1) * self.dim];
            let mut s = 0.0f32;
            for i in 0..self.dim { s += q[i] * row[i]; }
            if top.len() < k {
                top.push((self.ids[r], s));
                top.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            } else if s > top[0].1 {
                top[0] = (self.ids[r], s);
                let mut j = 0;
                while j + 1 < top.len() && top[j].1 > top[j + 1].1 { top.swap(j, j + 1); j += 1; }
            }
        }
        top.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap()); // best first
        top
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(seed: f32, dim: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim).map(|i| (seed + i as f32).sin()).collect();
        let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v { *x /= n; }
        v
    }

    #[test]
    fn ring_overwrites_oldest_at_cap() {
        let mut idx = EmbeddingIndex::new(4, 2);
        idx.append(Ulid::from(1u128), &unit(1.0, 4));
        idx.append(Ulid::from(2u128), &unit(2.0, 4));
        idx.append(Ulid::from(3u128), &unit(3.0, 4)); // evicts id 1
        assert_eq!(idx.len(), 2);
        let hits = idx.scan_topk(&unit(3.0, 4), 2);
        let ids: Vec<u128> = hits.iter().map(|(id, _)| u128::from(*id)).collect();
        assert!(ids.contains(&3) && ids.contains(&2));
        assert!(!ids.contains(&1), "oldest id 1 was overwritten");
    }

    #[test]
    fn scan_ranks_nearest_first() {
        let mut idx = EmbeddingIndex::new(4, 8);
        let q = unit(5.0, 4);
        idx.append(Ulid::from(10u128), &q); // identical → cosine ~1
        idx.append(Ulid::from(11u128), &unit(99.0, 4));
        let hits = idx.scan_topk(&q, 1);
        assert_eq!(u128::from(hits[0].0), 10);
        assert!(hits[0].1 > 0.99);
    }
}
