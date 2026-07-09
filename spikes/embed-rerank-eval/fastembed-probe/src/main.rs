//! Phase-0 musl LINK probe for the fastembed/ort (ONNX Runtime) path.
//!
//! Goal: force the linker to pull in `ort` (libonnxruntime, C++) so we learn
//! whether it links for `x86_64-unknown-linux-musl`. Guarded so a run does no
//! heavy work / no model download; the point is the LINK.

fn main() -> anyhow::Result<()> {
    if std::env::var("RUN_HEAVY").is_ok() {
        // Referencing TextEmbedding forces the ort native runtime to link.
        use fastembed::{InitOptions, EmbeddingModel, TextEmbedding};
        let _m = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15),
        );
    }
    println!("fastembed-probe: linked OK");
    Ok(())
}
