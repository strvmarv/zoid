//! Curated local model seed data — the source for `local_models` db rows.
//! Pure data: no deps (this crate is dependency-free by design). The bin maps
//! these structs to SQLite rows at seed time. User-defined entries
//! (`source = "user"`) are never overwritten by re-seeding.

/// One curated local model definition. All fields are `&'static str` or
/// integers so the const array is compile-time constructible with zero deps.
/// `vram_curve` is a JSON string literal — the bin stores it as-is in the db.
pub struct LocalModelSeed {
    pub id: &'static str,
    pub display_name: &'static str,
    pub provider: &'static str,
    pub runtime: &'static str,
    pub download_source: &'static str,
    pub quant: Option<&'static str>,
    pub modelfile: &'static str,
    pub context_window: u64,
    pub thinking: &'static str,
    pub thinking_wire: &'static str,
    pub max_output: u64,
    pub tools: bool,
    pub prompt_cache: bool,
    pub num_ctx: u32,
    pub vram_curve: &'static str,
    pub schema_version: u32,
}

/// The curated local model catalog. Start small — qwythos only (the one zoid
/// has validated end-to-end). Adding more models is incremental: add an entry,
/// bump its `schema_version`, and the seed step on the next boot updates the
/// db row.
pub const CURATED_LOCAL_MODELS: &[LocalModelSeed] = &[LocalModelSeed {
    id: "qwythos",
    display_name: "Qwythos 9B (Claude Mythos 5, 1M)",
    provider: "ollama-local",
    runtime: "ollama",
    download_source: "hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M",
    quant: Some("Q4_K_M"),
    modelfile: r#"FROM hf.co/empero-ai/Qwythos-9B-Claude-Mythos-5-1M-GGUF:Q4_K_M
TEMPLATE """{{ if .System }}<|im_start|>system
{{ .System }}<|im_end|>{{ end }}<|im_start|>user
{{ .Prompt }}<|im_end|>
<|im_start|>assistant"""
PARAMETER stop <|im_end|>
PARAMETER stop <|im_start|>"#,
    context_window: 1_048_576,
    thinking: "Toggle",
    thinking_wire: "Ollama",
    max_output: 0,
    tools: true,
    prompt_cache: true,
    num_ctx: 98_304,
    vram_curve: r#"[{"num_ctx":32768,"vram_mb":7000},{"num_ctx":65536,"vram_mb":8500},{"num_ctx":98304,"vram_mb":10000},{"num_ctx":131072,"vram_mb":12000}]"#,
    schema_version: 1,
}];
