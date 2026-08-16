//! Reciprocal Rank Fusion for hybrid recall. Merges two rank-ordered id lists
//! (FTS/BM25 and vector/cosine) by RANK, not raw score — so BM25's unbounded
//! scale and cosine's [-1,1] never need calibration. score = Σ 1/(k + rank_i).

use ulid::Ulid;

const RRF_K: f32 = 60.0;

pub fn hybrid_recall(fts_ids: &[Ulid], vector_ids: &[Ulid], limit: usize) -> Vec<Ulid> {
    use std::collections::HashMap;
    let mut score: HashMap<Ulid, f32> = HashMap::new();
    let mut order: Vec<Ulid> = Vec::new(); // first-seen order, for stable ties
    for list in [fts_ids, vector_ids] {
        for (rank, id) in list.iter().enumerate() {
            let e = score.entry(*id).or_insert_with(|| {
                order.push(*id);
                0.0
            });
            *e += 1.0 / (RRF_K + rank as f32);
        }
    }
    order.sort_by(|a, b| {
        score[b]
            .partial_cmp(&score[a])
            .unwrap()
            .then_with(|| a.cmp(b)) // deterministic tie-break by id
    });
    order.truncate(limit);
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u128) -> Ulid {
        Ulid::from(n)
    }

    #[test]
    fn identical_lists_preserve_order() {
        let a = [id(1), id(2), id(3)];
        assert_eq!(hybrid_recall(&a, &a, 10), a.to_vec());
    }

    #[test]
    fn empty_vector_equals_fts_order() {
        let fts = [id(5), id(6), id(7)];
        assert_eq!(hybrid_recall(&fts, &[], 10), fts.to_vec());
    }

    #[test]
    fn disjoint_lists_interleave_by_rank() {
        let fts = [id(1), id(2)];
        let vec = [id(3), id(4)];
        // rank-0 of each source tie (id1, id3), then rank-1 (id2, id4)
        let out = hybrid_recall(&fts, &vec, 10);
        assert_eq!(out.len(), 4);
        assert!(out[..2].contains(&id(1)) && out[..2].contains(&id(3)));
        assert!(out[2..].contains(&id(2)) && out[2..].contains(&id(4)));
    }

    #[test]
    fn shared_id_ranks_higher_than_singletons() {
        let fts = [id(1), id(9)];
        let vec = [id(2), id(9)];
        let out = hybrid_recall(&fts, &vec, 10);
        assert_eq!(out[0], id(9), "id present in both sources wins");
    }
}
