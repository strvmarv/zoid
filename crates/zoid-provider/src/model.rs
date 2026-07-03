//! Basic, caps-only model registry (spec 2026-07-01-model-registry.md): one
//! source of truth for known providers/models and per-model capabilities.
//! No cost/pricing (economy is tokens-only). Wire-derived caps (Ollama
//! /api/show) are a future refinement.

/// Stable, model-agnostic capabilities of a model. No cost fields by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub context_window: u64,
    pub max_output: u64, // 0 = "use provider default"
    pub tools: bool,
}

/// How a provider entry is reached. Http/Cli carry their default connection
/// value; Sdk has none (ambient auth). This is the growth seam for new
/// transports (spec 2026-07-03-settings-redesign).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Http { default_base_url: &'static str },
    Cli { default_command: &'static str },
    Sdk,
}

/// Whether an entry is implemented (selectable) or a visible-but-inert seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    Planned,
}

/// One provider flavor. `id` is a stable hyphenated `family-variant` key;
/// code reads these fields, never substring-parses `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEntry {
    pub id: &'static str,
    pub display: &'static str,
    pub family: &'static str,
    pub transport: Transport,
    pub models: &'static [&'static str],
    pub status: Status,
}

/// The provider registry. Order is the picker display order.
pub const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        id: "ollama-local",
        display: "ollama · local",
        family: "ollama",
        transport: Transport::Http { default_base_url: "http://localhost:11434" },
        models: &[], // local tags are arbitrary; free-text entry
        status: Status::Available,
    },
    ProviderEntry {
        id: "ollama-cloud",
        display: "ollama · cloud",
        family: "ollama",
        transport: Transport::Http { default_base_url: "https://ollama.com" },
        models: &["glm-5.2:cloud"],
        status: Status::Available,
    },
    ProviderEntry {
        id: "anthropic-api",
        display: "anthropic · api key",
        family: "anthropic",
        transport: Transport::Http { default_base_url: "https://api.anthropic.com" },
        models: &["claude-sonnet-4-6", "claude-opus-4-8"],
        status: Status::Available,
    },
    ProviderEntry {
        id: "anthropic-cli",
        display: "anthropic · Claude Code CLI",
        family: "anthropic",
        transport: Transport::Cli { default_command: "claude" },
        models: &["claude-sonnet-4-6", "claude-opus-4-8"],
        status: Status::Planned,
    },
    ProviderEntry {
        id: "anthropic-sdk",
        display: "anthropic · SDK",
        family: "anthropic",
        transport: Transport::Sdk,
        models: &["claude-sonnet-4-6", "claude-opus-4-8"],
        status: Status::Planned,
    },
];

/// Resolve a stored/legacy provider id to its canonical registry id.
/// Preserves today's behavior: bare `ollama` meant the cloud endpoint.
pub fn canonical_id(raw: &str) -> &str {
    match raw {
        "ollama" => "ollama-cloud",
        "anthropic" => "anthropic-api",
        other => other,
    }
}

/// The registry entry for a provider id (resolving legacy aliases).
pub fn entry(id: &str) -> Option<&'static ProviderEntry> {
    let id = canonical_id(id);
    PROVIDERS.iter().find(|e| e.id == id)
}

/// Known model ids for a provider (first = default). Empty for free-text-only
/// providers (local Ollama) or unknown ids. Resolves legacy aliases.
pub fn models_for(provider: &str) -> &'static [&'static str] {
    entry(provider).map(|e| e.models).unwrap_or(&[])
}

/// The registry default base URL for an HTTP-transport provider, else `None`.
pub fn default_base_url(provider: &str) -> Option<&'static str> {
    match entry(provider).map(|e| e.transport) {
        Some(Transport::Http { default_base_url }) => Some(default_base_url),
        _ => None,
    }
}

/// Iterator over selectable (Available) entries — `[planned]` excluded.
pub fn selectable() -> impl Iterator<Item = &'static ProviderEntry> {
    PROVIDERS.iter().filter(|e| e.status == Status::Available)
}

/// Capabilities for `model`, matched by family (case-insensitive), else DEFAULT.
pub fn model_info(model: &str) -> ModelInfo {
    let m = model.to_ascii_lowercase();
    // Claude is a known 200k family; everything else (incl. GLM, whose exact
    // window is a registry TODO) takes the 256k conservative default.
    let context_window = if m.contains("claude") {
        200_000
    } else {
        256_000
    };
    ModelInfo {
        context_window,
        max_output: 0,
        tools: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_info_caps_by_family_else_default() {
        assert_eq!(model_info("claude-sonnet-4-6").context_window, 200_000);
        assert_eq!(model_info("CLAUDE-opus").context_window, 200_000);
        assert_eq!(model_info("glm-5.2:cloud").context_window, 256_000);
        assert_eq!(model_info("llama3.1:70b").context_window, 256_000);
        assert!(model_info("anything").tools);
    }

    #[test]
    fn canonical_id_maps_legacy_aliases() {
        assert_eq!(canonical_id("ollama"), "ollama-cloud");
        assert_eq!(canonical_id("anthropic"), "anthropic-api");
        assert_eq!(canonical_id("ollama-local"), "ollama-local"); // pass-through
        assert_eq!(canonical_id("unknown"), "unknown");
    }

    #[test]
    fn entry_resolves_through_alias_and_transport() {
        let e = entry("ollama").unwrap(); // legacy → ollama-cloud
        assert_eq!(e.id, "ollama-cloud");
        assert_eq!(e.family, "ollama");
        assert_eq!(e.transport, Transport::Http { default_base_url: "https://ollama.com" });

        let local = entry("ollama-local").unwrap();
        assert_eq!(local.transport, Transport::Http { default_base_url: "http://localhost:11434" });

        let cli = entry("anthropic-cli").unwrap();
        assert_eq!(cli.transport, Transport::Cli { default_command: "claude" });
        assert_eq!(cli.status, Status::Planned);
    }

    #[test]
    fn models_for_by_id_and_alias() {
        assert_eq!(models_for("ollama"), &["glm-5.2:cloud"]); // alias → cloud
        assert_eq!(models_for("ollama-cloud"), &["glm-5.2:cloud"]);
        assert!(models_for("ollama-local").is_empty()); // local tags are free-text
        assert!(models_for("anthropic-api").contains(&"claude-sonnet-4-6"));
        assert!(models_for("nonexistent").is_empty());
    }

    #[test]
    fn default_base_url_only_for_http() {
        assert_eq!(default_base_url("anthropic-api"), Some("https://api.anthropic.com"));
        assert_eq!(default_base_url("anthropic-cli"), None); // Cli has no url
        assert_eq!(default_base_url("anthropic-sdk"), None);
    }

    #[test]
    fn selectable_excludes_planned() {
        let ids: Vec<&str> = selectable().map(|e| e.id).collect();
        assert!(ids.contains(&"ollama-local"));
        assert!(ids.contains(&"ollama-cloud"));
        assert!(ids.contains(&"anthropic-api"));
        assert!(!ids.contains(&"anthropic-cli"));
        assert!(!ids.contains(&"anthropic-sdk"));
    }
}
