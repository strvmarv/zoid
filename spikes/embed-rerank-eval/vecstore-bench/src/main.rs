//! Synthetic bench for the EmbeddingIndex resume-load + brute-force cosine scan.
//! Random 384-d unit vectors in a real on-disk sqlite table. No ML models.
//!
//! For each N: build a fresh file DB, insert N unit vectors, reopen (cold cache
//! best-effort), then measure:
//!   (1) LOAD: `ORDER BY rowid DESC LIMIT cap` -> flat Vec<f32>  (resume fill)
//!   (2) SCAN: top-20 cosine (= dot, unit vecs) over the loaded matrix, cold+warm

use anyhow::Result;
use rand::Rng;
use rusqlite::Connection;
use std::time::Instant;

const DIM: usize = 384;
const TOPK: usize = 20;

fn unit_vec(rng: &mut impl Rng) -> Vec<f32> {
    let mut v: Vec<f32> = (0..DIM).map(|_| rng.gen_range(-1.0f32..1.0)).collect();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for f in v {
        b.extend_from_slice(&f.to_le_bytes());
    }
    b
}

fn scan_topk(q: &[f32], vectors: &[f32], n: usize) -> Vec<(usize, f32)> {
    // min-heap of size TOPK via a simple bounded insert (K is tiny).
    let mut top: Vec<(usize, f32)> = Vec::with_capacity(TOPK + 1);
    for r in 0..n {
        let row = &vectors[r * DIM..(r + 1) * DIM];
        let mut s = 0.0f32;
        for i in 0..DIM {
            s += q[i] * row[i];
        }
        if top.len() < TOPK {
            top.push((r, s));
            if top.len() == TOPK {
                top.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            }
        } else if s > top[0].1 {
            top[0] = (r, s);
            // keep the smallest at front
            let mut j = 0;
            while j + 1 < top.len() && top[j].1 > top[j + 1].1 {
                top.swap(j, j + 1);
                j += 1;
            }
        }
    }
    top
}

fn bench_n(n: usize, cap: usize) -> Result<()> {
    let dir = std::env::temp_dir().join(format!("vecbench-{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let db_path = dir.join("ev.db");

    // ---- build DB ----
    let mut rng = rand::thread_rng();
    {
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE event_embeddings(
               event_id INTEGER PRIMARY KEY, model_id TEXT, dim INTEGER, vector BLOB);",
        )?;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO event_embeddings(event_id,model_id,dim,vector) VALUES(?,?,?,?)",
            )?;
            for id in 0..n {
                let v = unit_vec(&mut rng);
                stmt.execute(rusqlite::params![id as i64, "bge-small", DIM as i64, to_blob(&v)])?;
            }
        }
        tx.commit()?;
    }
    let db_bytes = std::fs::metadata(&db_path)?.len()
        + std::fs::metadata(dir.join("ev.db-wal")).map(|m| m.len()).unwrap_or(0);

    // reopen fresh connection (best-effort cold-ish)
    let conn = Connection::open(&db_path)?;

    // ---- (1) LOAD recent-cap into flat matrix ----
    let load_cap = cap.min(n);
    let t = Instant::now();
    let mut vectors: Vec<f32> = Vec::with_capacity(load_cap * DIM);
    let mut ids: Vec<i64> = Vec::with_capacity(load_cap);
    {
        let mut stmt = conn.prepare(
            "SELECT event_id, vector FROM event_embeddings
             WHERE model_id='bge-small' ORDER BY rowid DESC LIMIT ?",
        )?;
        let mut rows = stmt.query([load_cap as i64])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            ids.push(id);
            for c in blob.chunks_exact(4) {
                vectors.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }
    }
    let load_ms = t.elapsed().as_secs_f64() * 1e3;
    let loaded = ids.len();

    // ---- (2) SCAN top-K cosine ----
    let q = unit_vec(&mut rng);
    let t = Instant::now();
    let _cold = scan_topk(&q, &vectors, loaded);
    let scan_cold_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let _warm = scan_topk(&q, &vectors, loaded);
    let scan_warm_ms = t.elapsed().as_secs_f64() * 1e3;

    let ram_mb = (vectors.len() * 4) as f64 / (1024.0 * 1024.0);
    let db_mb = db_bytes as f64 / (1024.0 * 1024.0);
    println!(
        "N={:>7}  cap={:>7}  loaded={:>7}  DB={:>6.1}MB  RAM={:>6.1}MB  | LOAD {:>7.1}ms  | SCAN cold {:>6.3}ms  warm {:>6.3}ms",
        n, cap, loaded, db_mb, ram_mb, load_ms, scan_cold_ms, scan_warm_ms
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

fn main() -> Result<()> {
    println!("== vecstore bench (dim={DIM}, topK={TOPK}) ==");
    println!("threads: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    let cap = 150_000;
    for n in [10_000usize, 50_000, 150_000, 300_000] {
        bench_n(n, cap)?;
    }
    println!("== done ==");
    Ok(())
}
