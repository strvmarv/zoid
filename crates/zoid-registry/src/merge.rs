//! Merge user registry patch over shipped registry.

use zoid_model::{
    ModelEntry, ModelInfo, ModelPatch, ProviderEntry, Registry, RegistryPatch, Source, Status,
    ThinkingSupport, ThinkingWireShape, Transport, WireShape,
};

/// Merge `user` over `shipped`. User patches override shipped rows by
/// `(provider.id, model.id)` (case-insensitive on model id), field-by-field:
/// only the fields the user set are applied; everything else keeps the shipped
/// value. A user patch may add a new provider or model. `hidden = true` hides a
/// shipped model. A user `default = true` demotes the shipped default.
pub fn merge(shipped: Registry, user: RegistryPatch) -> Registry {
    let mut providers: Vec<ProviderEntry> = shipped.providers;

    for up in user.providers {
        match providers.iter_mut().find(|p| p.id == up.id) {
            Some(existing) => {
                for um in up.models {
                    let key = um.id.to_ascii_lowercase();
                    let idx = existing
                        .models
                        .iter()
                        .position(|m| m.id.to_ascii_lowercase() == key);
                    match idx {
                        Some(i) => {
                            // Existing model: apply the patch field-by-field. If
                            // the user sets default = true, demote the previous
                            // default among the siblings first (so only one
                            // default survives), then apply the rest of the patch.
                            if um.default == Some(true) {
                                for m in existing.models.iter_mut() {
                                    m.default = false;
                                }
                            }
                            apply_patch(&mut existing.models[i], um);
                        }
                        None => {
                            // New model: build a full ModelEntry from the patch,
                            // defaulting unset fields to conservative values.
                            let entry = ModelEntry {
                                id: um.id.clone(),
                                display: um.display.clone(),
                                wire_shape: um.wire_shape.unwrap_or(WireShape::OpenAIChat),
                                source: um.source.unwrap_or(Source::User),
                                default: um.default.unwrap_or(false),
                                hidden: um.hidden.unwrap_or(false),
                                info: ModelInfo {
                                    context_window: um.context_window.unwrap_or(32_000),
                                    max_output: um.max_output.unwrap_or(0),
                                    tools: um.tools.unwrap_or(true),
                                    prompt_cache: um.prompt_cache.unwrap_or(false),
                                    thinking: um.thinking.unwrap_or(ThinkingSupport::None),
                                    thinking_wire: um
                                        .thinking_wire
                                        .unwrap_or(ThinkingWireShape::None),
                                },
                                runtime: um.runtime.clone(),
                                download_source: um.download_source.clone(),
                                quant: um.quant.clone(),
                                modelfile: um.modelfile.clone(),
                                num_ctx: um.num_ctx,
                                vram_curve: um.vram_curve.clone(),
                            };
                            if entry.default {
                                for m in existing.models.iter_mut() {
                                    m.default = false;
                                }
                            }
                            existing.models.push(entry);
                        }
                    }
                }
            }
            None => {
                // New provider: build full ModelEntries from patches.
                // NOTE: a user-added provider is always keyless with an empty
                // base URL (ProviderPatch carries only id + models). This is a
                // documented limitation — user-added providers are for local/
                // keyless use; keyed providers must be shipped. Also demote any
                // duplicate `default = true` among the new provider's own models
                // (keep the first, clear the rest).
                let mut models = Vec::new();
                let mut saw_default = false;
                for um in up.models {
                    let mut is_default = um.default.unwrap_or(false);
                    if is_default && saw_default {
                        is_default = false;
                    }
                    if is_default {
                        saw_default = true;
                    }
                    models.push(ModelEntry {
                        id: um.id.clone(),
                        display: um.display.clone(),
                        wire_shape: um.wire_shape.unwrap_or(WireShape::OpenAIChat),
                        source: um.source.unwrap_or(Source::User),
                        default: is_default,
                        hidden: um.hidden.unwrap_or(false),
                        info: ModelInfo {
                            context_window: um.context_window.unwrap_or(32_000),
                            max_output: um.max_output.unwrap_or(0),
                            tools: um.tools.unwrap_or(true),
                            prompt_cache: um.prompt_cache.unwrap_or(false),
                            thinking: um.thinking.unwrap_or(ThinkingSupport::None),
                            thinking_wire: um.thinking_wire.unwrap_or(ThinkingWireShape::None),
                        },
                        runtime: um.runtime.clone(),
                        download_source: um.download_source.clone(),
                        quant: um.quant.clone(),
                        modelfile: um.modelfile.clone(),
                        num_ctx: um.num_ctx,
                        vram_curve: um.vram_curve.clone(),
                    });
                }
                providers.push(ProviderEntry {
                    id: up.id.clone(),
                    display: up.id.clone(),
                    family: up.id.clone(),
                    transport: Transport::Http {
                        default_base_url: String::new(),
                    },
                    status: Status::Available,
                    key_url: None,
                    key_env: None,
                    models,
                });
            }
        }
    }

    Registry { providers }
}

/// Apply a patch to an existing model entry, field-by-field. Only the fields
/// the user set (`Some`) are applied; everything else keeps the shipped value.
/// Default demotion (clearing the previous default when the user sets
/// `default = true`) is handled by the caller before this runs, so this fn
/// only assigns `default` when the patch sets it.
fn apply_patch(em: &mut ModelEntry, um: ModelPatch) {
    if let Some(v) = um.display {
        em.display = Some(v);
    }
    if let Some(v) = um.wire_shape {
        em.wire_shape = v;
    }
    if let Some(v) = um.source {
        em.source = v;
    }
    if let Some(v) = um.hidden {
        em.hidden = v;
    }
    if let Some(v) = um.context_window {
        em.info.context_window = v;
    }
    if let Some(v) = um.max_output {
        em.info.max_output = v;
    }
    if let Some(v) = um.tools {
        em.info.tools = v;
    }
    if let Some(v) = um.prompt_cache {
        em.info.prompt_cache = v;
    }
    if let Some(v) = um.thinking {
        em.info.thinking = v;
    }
    if let Some(v) = um.thinking_wire {
        em.info.thinking_wire = v;
    }
    if let Some(v) = um.runtime {
        em.runtime = Some(v);
    }
    if let Some(v) = um.download_source {
        em.download_source = Some(v);
    }
    if let Some(v) = um.quant {
        em.quant = Some(v);
    }
    if let Some(v) = um.modelfile {
        em.modelfile = Some(v);
    }
    if let Some(v) = um.num_ctx {
        em.num_ctx = Some(v);
    }
    if let Some(v) = um.vram_curve {
        em.vram_curve = Some(v);
    }
    if let Some(v) = um.default {
        em.default = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_model::ProviderPatch;

    fn model(id: &str, default: bool, hidden: bool) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            display: None,
            wire_shape: WireShape::AnthropicMessages,
            source: Source::Static,
            default,
            hidden,
            info: ModelInfo {
                context_window: 1_000_000,
                max_output: 0,
                tools: true,
                prompt_cache: true,
                thinking: ThinkingSupport::Budget,
                thinking_wire: ThinkingWireShape::Anthropic,
            },
            runtime: None,
            download_source: None,
            quant: None,
            modelfile: None,
            num_ctx: None,
            vram_curve: None,
        }
    }

    fn provider(id: &str, models: Vec<ModelEntry>) -> ProviderEntry {
        ProviderEntry {
            id: id.to_string(),
            display: id.to_string(),
            family: id.to_string(),
            transport: Transport::Http {
                default_base_url: "https://x".to_string(),
            },
            status: Status::Available,
            key_url: Some("https://x".to_string()),
            key_env: Some("K".to_string()),
            models,
        }
    }

    fn patch(id: &str, hidden: Option<bool>, ctx: Option<u64>) -> ModelPatch {
        ModelPatch {
            id: id.to_string(),
            hidden,
            context_window: ctx,
            ..Default::default()
        }
    }

    #[test]
    fn partial_patch_preserves_shipped_caps() {
        // A user hides a model WITHOUT setting caps — the shipped 1M/Anthropic
        // caps must survive, not be clobbered to 32k/openai-chat.
        let shipped = Registry {
            providers: vec![provider("p", vec![model("a", true, false)])],
        };
        let user = RegistryPatch {
            providers: vec![ProviderPatch {
                id: "p".to_string(),
                models: vec![patch("a", Some(true), None)],
            }],
        };
        let merged = merge(shipped, user);
        let m = &merged.providers[0].models[0];
        assert!(m.hidden);
        assert_eq!(m.info.context_window, 1_000_000);
        assert_eq!(m.wire_shape, WireShape::AnthropicMessages);
        assert_eq!(m.info.thinking, ThinkingSupport::Budget);
    }

    #[test]
    fn explicit_override_changes_only_that_field() {
        let shipped = Registry {
            providers: vec![provider("p", vec![model("a", true, false)])],
        };
        let user = RegistryPatch {
            providers: vec![ProviderPatch {
                id: "p".to_string(),
                models: vec![patch("a", None, Some(999_999))],
            }],
        };
        let merged = merge(shipped, user);
        let m = &merged.providers[0].models[0];
        assert_eq!(m.info.context_window, 999_999);
        assert_eq!(m.wire_shape, WireShape::AnthropicMessages); // untouched
        assert!(m.default); // untouched
    }

    #[test]
    fn user_default_demotes_shipped_default() {
        let shipped = Registry {
            providers: vec![provider(
                "p",
                vec![model("a", true, false), model("b", false, false)],
            )],
        };
        let mut p = patch("b", None, None);
        p.default = Some(true);
        let user = RegistryPatch {
            providers: vec![ProviderPatch {
                id: "p".to_string(),
                models: vec![p],
            }],
        };
        let merged = merge(shipped, user);
        let defaults: Vec<&str> = merged.providers[0]
            .models
            .iter()
            .filter(|m| m.default)
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(defaults, vec!["b"]);
    }

    #[test]
    fn user_default_false_un_defaults_shipped_default() {
        // A user who explicitly writes `default = false` on a shipped
        // `default = true` model un-defaults it (field-by-field principle:
        // a set field must be applied, even when false).
        let shipped = Registry {
            providers: vec![provider("p", vec![model("a", true, false)])],
        };
        let mut p = patch("a", None, None);
        p.default = Some(false);
        let user = RegistryPatch {
            providers: vec![ProviderPatch {
                id: "p".to_string(),
                models: vec![p],
            }],
        };
        let merged = merge(shipped, user);
        let m = &merged.providers[0].models[0];
        assert!(!m.default);
    }

    #[test]
    fn user_can_add_new_provider() {
        let shipped = Registry {
            providers: vec![provider("p", vec![])],
        };
        let user = RegistryPatch {
            providers: vec![ProviderPatch {
                id: "q".to_string(),
                models: vec![patch("x", None, None)],
            }],
        };
        let merged = merge(shipped, user);
        assert_eq!(merged.providers.len(), 2);
        assert!(merged.entry("q").is_some());
    }
}
