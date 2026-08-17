//! Fetch + reconcile: regenerate wire rows from live endpoints.
//!
//! The reconcile loop compares the on-disk registry against live model lists
//! fetched from each provider and produces a [`ReconcileReport`] describing
//! what changed (and what needs a human's attention). It is kept
//! network-free-testable via the [`Fetcher`] seam: tests inject a mock
//! fetcher, the real binary injects [`ReqwestFetcher`].
//!
//! Semantics (spec §7 "Refresh-time"):
//! - A provider with no key in `keys` is skipped + reported.
//! - One provider's fetch failure is reported; other providers still
//!   reconcile (error isolation per provider).
//! - `wire_capable` providers (`ollama-cloud`, `ollama-local`, `gemini-api`)
//!   mutate the registry: new live models become `wire` rows (caps fetched),
//!   changed wire rows are updated, wire rows absent from live are removed.
//! - All other providers are report-only: new live models and absent
//!   static/user models are reported but never deleted.

use anyhow::Result;
use std::collections::HashMap;
use zoid_model::{ModelInfo, Registry, Source, Transport};

/// A report of what reconcile did (and what it left for a human).
#[derive(Debug, Default)]
pub struct ReconcileReport {
    /// `(provider, model)` pairs newly added as `wire` rows.
    pub added: Vec<(String, String)>,
    /// `(provider, model)` pairs whose caps changed (wire rows updated).
    pub updated: Vec<(String, String)>,
    /// `(provider, model)` wire rows absent from the live list (removed).
    pub removed: Vec<(String, String)>,
    /// Human-actionable notes (new models on report-only providers, absent
    /// static/user models, caps-fetch failures, etc.).
    pub reported: Vec<String>,
    /// Providers skipped (no key / non-HTTP transport / fetch error).
    pub skipped: Vec<String>,
    /// New caps for added and updated wire rows, keyed by `(provider, model)`.
    /// Consumed by `write_user_file` (Task 14) to serialize the refreshed
    /// values without re-fetching.
    pub caps: HashMap<(String, String), ModelInfo>,
}

/// Seam for fetching live model lists + caps. Mockable in tests; the real
/// binary uses [`ReqwestFetcher`].
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    /// Fetch the live model id list for a provider.
    async fn list(&self, provider: &str, base_url: &str, key: &str) -> Result<Vec<String>>;
    /// Fetch wire-derived caps for one model, or `None` when the provider
    /// exposes no caps endpoint for that model.
    async fn caps(
        &self,
        provider: &str,
        base_url: &str,
        key: &str,
        model: &str,
    ) -> Result<Option<ModelInfo>>;
}

/// Real `Fetcher` backed by `crate::fetch` (reqwest).
pub struct ReqwestFetcher;

#[async_trait::async_trait]
impl Fetcher for ReqwestFetcher {
    async fn list(&self, p: &str, b: &str, k: &str) -> Result<Vec<String>> {
        crate::fetch::list_models(p, b, k).await
    }
    async fn caps(&self, p: &str, b: &str, k: &str, m: &str) -> Result<Option<ModelInfo>> {
        crate::fetch::caps(p, b, k, m).await
    }
}

/// Whether a provider id is "wire-capable" — i.e. reconcile may add/update/
/// remove `wire` rows for it. Only Ollama (cloud/local) and Gemini expose a
/// caps endpoint we can derive `ModelInfo` from; all other providers are
/// report-only.
fn wire_capable(id: &str) -> bool {
    id == "ollama-cloud" || id == "ollama-local" || id == "gemini-api"
}

/// Reconcile the registry against live endpoints.
///
/// `keys` maps provider id → API key. Providers missing from `keys` (or with
/// an empty key) are skipped + reported. Per-provider fetch errors are
/// isolated: one failure does not abort the run.
///
/// Only wire-capable providers (Ollama cloud/local, Gemini) mutate the
/// registry; other providers are report-only.
pub async fn reconcile(
    reg: &Registry,
    keys: &HashMap<String, String>,
    fetcher: &dyn Fetcher,
) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();

    for p in &reg.providers {
        // Skip providers with no key (missing or empty).
        let key = match keys.get(&p.id) {
            Some(k) if !k.is_empty() => k,
            _ => {
                report.skipped.push(format!("{}: no key", p.id));
                continue;
            }
        };

        // Only HTTP-transport providers have a base URL to fetch from.
        let base_url = match &p.transport {
            Transport::Http { default_base_url } => default_base_url.clone(),
            _ => {
                report.skipped.push(format!("{}: non-HTTP", p.id));
                continue;
            }
        };

        // Fetch the live list; isolate failures to this provider.
        let live = match fetcher.list(&p.id, &base_url, key).await {
            Ok(l) => l,
            Err(e) => {
                report
                    .skipped
                    .push(format!("{}: fetch error: {e}", p.id));
                continue;
            }
        };
        let live_lower: Vec<String> = live.iter().map(|s| s.to_ascii_lowercase()).collect();

        if wire_capable(&p.id) {
            // --- Add: new live models become wire rows (with caps). ---
            for id in &live {
                let exists = p
                    .models
                    .iter()
                    .any(|m| m.id.to_ascii_lowercase() == id.to_ascii_lowercase());
                if !exists {
                    report.added.push((p.id.clone(), id.clone()));
                    match fetcher.caps(&p.id, &base_url, key, id).await {
                        Ok(Some(info)) => {
                            report.caps.insert((p.id.clone(), id.clone()), info);
                        }
                        Ok(None) => {
                            report
                                .reported
                                .push(format!("{}: no caps for new model {} (using defaults)", p.id, id));
                        }
                        Err(e) => {
                            report.reported.push(format!(
                                "{}: caps fetch error for new model {}: {e}",
                                p.id, id
                            ));
                        }
                    }
                }
            }

            // --- Update: existing wire rows whose caps changed. ---
            for m in &p.models {
                if m.source != Source::Wire {
                    continue;
                }
                match fetcher.caps(&p.id, &base_url, key, &m.id).await {
                    Ok(Some(new_info)) => {
                        if new_info != m.info {
                            report.updated.push((p.id.clone(), m.id.clone()));
                            report.caps.insert((p.id.clone(), m.id.clone()), new_info);
                        }
                    }
                    Ok(None) => {
                        report
                            .reported
                            .push(format!("{}: couldn't refresh caps for {}", p.id, m.id));
                    }
                    Err(e) => {
                        report
                            .reported
                            .push(format!("{}: caps fetch error for {}: {e}", p.id, m.id));
                    }
                }
            }

            // --- Remove: wire rows absent from the live list. ---
            for m in &p.models {
                if m.source == Source::Wire && !live_lower.contains(&m.id.to_ascii_lowercase()) {
                    report.removed.push((p.id.clone(), m.id.clone()));
                }
            }
        } else {
            // --- Report-only: new live models + absent static/user models. ---
            for id in &live {
                let exists = p
                    .models
                    .iter()
                    .any(|m| m.id.to_ascii_lowercase() == id.to_ascii_lowercase());
                if !exists {
                    report
                        .reported
                        .push(format!("{}: new model {} (needs manual caps)", p.id, id));
                }
            }
            for m in &p.models {
                if !live_lower.contains(&m.id.to_ascii_lowercase()) {
                    report.reported.push(format!(
                        "{}: model {} absent from live (static/user, not removed)",
                        p.id, m.id
                    ));
                }
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_model::{ModelEntry, ModelInfo, ProviderEntry, Registry, Source, Status, Transport, WireShape};

    /// A minimal `ModelInfo` for tests (zeroed caps).
    fn info() -> ModelInfo {
        ModelInfo {
            context_window: 0,
            max_output: 0,
            tools: false,
            prompt_cache: false,
            thinking: zoid_model::ThinkingSupport::None,
            thinking_wire: zoid_model::ThinkingWireShape::None,
        }
    }

    fn static_model(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            display: None,
            wire_shape: WireShape::Ollama,
            source: Source::Static,
            default: false,
            hidden: false,
            info: info(),
            runtime: None,
            download_source: None,
            quant: None,
            modelfile: None,
            num_ctx: None,
            vram_curve: None,
        }
    }

    fn http_provider(id: &str, models: Vec<ModelEntry>) -> ProviderEntry {
        ProviderEntry {
            id: id.to_string(),
            display: id.to_string(),
            family: id.to_string(),
            transport: Transport::Http {
                default_base_url: format!("https://{id}.example.com"),
            },
            status: Status::Available,
            key_url: None,
            key_env: None,
            models,
        }
    }

    /// A mock fetcher that returns canned lists and always `None` caps.
    struct MockFetcher {
        lists: HashMap<String, Vec<String>>,
        caps_map: HashMap<String, ModelInfo>,
    }

    #[async_trait::async_trait]
    impl Fetcher for MockFetcher {
        async fn list(&self, p: &str, _b: &str, _k: &str) -> Result<Vec<String>> {
            Ok(self.lists.get(p).cloned().unwrap_or_default())
        }
        async fn caps(&self, p: &str, _b: &str, _k: &str, m: &str) -> Result<Option<ModelInfo>> {
            Ok(self.caps_map.get(&format!("{p}/{m}")).copied())
        }
    }

    #[tokio::test]
    async fn reconcile_adds_wire_for_ollama_reports_for_anthropic() {
        // ollama-cloud (wire-capable) has one existing static model that is
        // also in the live list, plus a brand-new live model. anthropic-api
        // (report-only) has one existing static model plus a new live model.
        let reg = Registry {
            providers: vec![
                http_provider(
                    "ollama-cloud",
                    vec![static_model("glm-5.2:cloud")],
                ),
                http_provider(
                    "anthropic-api",
                    vec![static_model("claude-sonnet-4-6")],
                ),
            ],
        };

        let mut lists = HashMap::new();
        lists.insert(
            "ollama-cloud".to_string(),
            vec!["glm-5.2:cloud".to_string(), "new-cloud-model".to_string()],
        );
        lists.insert(
            "anthropic-api".to_string(),
            vec![
                "claude-sonnet-4-6".to_string(),
                "new-anthropic-model".to_string(),
            ],
        );

        let keys = HashMap::from([
            ("ollama-cloud".to_string(), "k".to_string()),
            ("anthropic-api".to_string(), "k".to_string()),
        ]);

        let report = reconcile(&reg, &keys, &MockFetcher { lists, caps_map: HashMap::new() })
            .await
            .unwrap();

        // ollama-cloud: the new live model is added as a wire row.
        assert!(report
            .added
            .contains(&("ollama-cloud".to_string(), "new-cloud-model".to_string())));
        // The existing static model is NOT re-added (it's already present).
        assert!(!report
            .added
            .iter()
            .any(|(_, m)| m == "glm-5.2:cloud"));

        // anthropic-api: the new model is reported, NOT added.
        assert!(report.reported.iter().any(|s| s.contains("new-anthropic-model")));
        assert!(!report.added.iter().any(|(p, _)| p == "anthropic-api"));

        // The existing static model on anthropic-api is present in live, so it
        // is neither reported-absent nor added.
        assert!(!report
            .reported
            .iter()
            .any(|s| s.contains("claude-sonnet-4-6") && s.contains("absent")));
    }

    #[tokio::test]
    async fn reconcile_skips_provider_with_no_key() {
        let reg = Registry {
            providers: vec![http_provider("ollama-cloud", vec![])],
        };
        let keys = HashMap::new();
        let report = reconcile(&reg, &keys, &MockFetcher {
            lists: HashMap::new(),
            caps_map: HashMap::new(),
        })
        .await
        .unwrap();
        assert!(report.skipped.iter().any(|s| s.contains("ollama-cloud") && s.contains("no key")));
        assert!(report.added.is_empty());
    }

    #[tokio::test]
    async fn reconcile_isolates_fetch_error() {
        // A fetcher whose list() always errors for one provider.
        struct ErrorFetcher;
        #[async_trait::async_trait]
        impl Fetcher for ErrorFetcher {
            async fn list(&self, _p: &str, _b: &str, _k: &str) -> Result<Vec<String>> {
                anyhow::bail!("network down")
            }
            async fn caps(&self, _p: &str, _b: &str, _k: &str, _m: &str) -> Result<Option<ModelInfo>> {
                Ok(None)
            }
        }
        let reg = Registry {
            providers: vec![http_provider("ollama-cloud", vec![])],
        };
        let keys = HashMap::from([("ollama-cloud".to_string(), "k".to_string())]);
        let report = reconcile(&reg, &keys, &ErrorFetcher).await.unwrap();
        assert!(report.skipped.iter().any(|s| s.contains("fetch error") && s.contains("network down")));
        assert!(report.added.is_empty());
    }

    #[tokio::test]
    async fn reconcile_removes_absent_wire_rows() {
        // ollama-cloud has a wire row that is NOT in the live list → removed.
        let mut wire_row = static_model("ghost-model");
        wire_row.source = Source::Wire;
        let reg = Registry {
            providers: vec![http_provider("ollama-cloud", vec![wire_row])],
        };
        let mut lists = HashMap::new();
        lists.insert("ollama-cloud".to_string(), vec!["some-other-model".to_string()]);
        let keys = HashMap::from([("ollama-cloud".to_string(), "k".to_string())]);
        let report = reconcile(&reg, &keys, &MockFetcher { lists, caps_map: HashMap::new() })
            .await
            .unwrap();
        assert!(report.removed.contains(&("ollama-cloud".to_string(), "ghost-model".to_string())));
    }

    #[tokio::test]
    async fn reconcile_updates_changed_wire_caps() {
        // An existing wire row whose live caps differ from the registry → updated.
        let mut wire_row = static_model("glm-5.2:cloud");
        wire_row.source = Source::Wire;
        wire_row.info = info(); // zeroed
        let reg = Registry {
            providers: vec![http_provider("ollama-cloud", vec![wire_row])],
        };
        let mut lists = HashMap::new();
        lists.insert("ollama-cloud".to_string(), vec!["glm-5.2:cloud".to_string()]);
        let mut caps_map = HashMap::new();
        caps_map.insert(
            "ollama-cloud/glm-5.2:cloud".to_string(),
            ModelInfo {
                context_window: 131072,
                ..info()
            },
        );
        let keys = HashMap::from([("ollama-cloud".to_string(), "k".to_string())]);
        let report = reconcile(&reg, &keys, &MockFetcher { lists, caps_map }).await.unwrap();
        assert!(report.updated.contains(&("ollama-cloud".to_string(), "glm-5.2:cloud".to_string())));
        let caps = report
            .caps
            .get(&("ollama-cloud".to_string(), "glm-5.2:cloud".to_string()))
            .unwrap();
        assert_eq!(caps.context_window, 131072);
    }

    #[tokio::test]
    async fn reconcile_does_not_update_unchanged_wire_caps() {
        let mut wire_row = static_model("glm-5.2:cloud");
        wire_row.source = Source::Wire;
        let reg = Registry {
            providers: vec![http_provider("ollama-cloud", vec![wire_row.clone()])],
        };
        let mut lists = HashMap::new();
        lists.insert("ollama-cloud".to_string(), vec!["glm-5.2:cloud".to_string()]);
        // Return the SAME caps as the registry row.
        let mut caps_map = HashMap::new();
        caps_map.insert("ollama-cloud/glm-5.2:cloud".to_string(), wire_row.info);
        let keys = HashMap::from([("ollama-cloud".to_string(), "k".to_string())]);
        let report = reconcile(&reg, &keys, &MockFetcher { lists, caps_map }).await.unwrap();
        assert!(report.updated.is_empty());
    }

    #[tokio::test]
    async fn reconcile_reports_absent_static_model_on_report_only_provider() {
        // anthropic-api (report-only) has a static model NOT in the live list.
        let reg = Registry {
            providers: vec![http_provider("anthropic-api", vec![static_model("old-model")])],
        };
        let mut lists = HashMap::new();
        lists.insert("anthropic-api".to_string(), vec!["claude-sonnet-4-6".to_string()]);
        let keys = HashMap::from([("anthropic-api".to_string(), "k".to_string())]);
        let report = reconcile(&reg, &keys, &MockFetcher { lists, caps_map: HashMap::new() })
            .await
            .unwrap();
        assert!(report
            .reported
            .iter()
            .any(|s| s.contains("old-model") && s.contains("absent from live")));
        // It is NOT removed (report-only provider).
        assert!(report.removed.is_empty());
    }
}