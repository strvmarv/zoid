//! Phase-0 musl LINK probe for the pure-Rust (candle) path.
//!
//! Goal: force the linker to pull in candle's CPU backend + tokenizers so we
//! learn whether the graph links for `x86_64-unknown-linux-musl`. We reference
//! the heavy APIs behind a runtime flag so `cargo build` must link them, but a
//! plain run does no real work (no model download).

use candle_core::{Device, Tensor};

fn main() -> anyhow::Result<()> {
    // Reachable-but-guarded: keeps the code linked without executing heavy work.
    if std::env::var("RUN_HEAVY").is_ok() {
        let dev = Device::Cpu;
        // A trivial op references the CPU backend kernels (the thing that must
        // link on musl).
        let a = Tensor::randn(0f32, 1f32, (8, 384), &dev)?;
        let b = a.matmul(&a.t()?)?;
        println!("{:?}", b.dims());

        // Reference the BERT model type + tokenizer so their code links too.
        let _ = candle_transformers::models::bert::Config::default();
        let _tok = tokenizers::Tokenizer::from_bytes(b"{}").ok();
    }
    println!("candle-probe: linked OK");
    Ok(())
}
