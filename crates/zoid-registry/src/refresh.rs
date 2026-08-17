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

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use zoid_model::{ModelInfo, Registry, Source, Transport, WireShape};

use crate::raw::{RawModelSer, RawProviderSer, RawRegistrySer};

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

/// Write the refreshed `wire` rows to `models.user.toml`, preserving every
/// existing `user` row verbatim. The apply rules (spec §7 "Refresh-time"):
///
/// 1. Read the existing user file (if present) and keep every row that is NOT
///    a `wire` row being touched by the report. `user` rows (and wire rows not
///    in `report.added`/`updated`/`removed`) are carried over unchanged.
/// 2. For each `report.added` entry, append a `wire` row using the caps in
///    `report.caps` (populated by `reconcile`); when caps are absent (the
///    fetch returned `None`), emit a conservative-default wire row.
/// 3. For each `report.updated` entry, rewrite that `wire` row's caps from
///    `report.caps`.
/// 4. For each `report.removed` entry, drop that `wire` row.
/// 5. Serialize to TOML and write atomically (temp file + rename) so a crash
///    mid-write never leaves a half-written user file.
///
/// `reg` is the *merged* registry (`zoid_registry::load`'s output) — it's used
/// to look up the existing wire row's `wire_shape` for `added`/`updated` rows
/// (the shape is provider/model metadata, not endpoint-derived, and lives in
/// the shipped file for the curated providers).
pub fn write_user_file(
    user_path: &Path,
    reg: &Registry,
    report: &ReconcileReport,
) -> Result<()> {
    // Index the report entries for O(1) lookup by (provider, model).
    let updated: HashMap<(String, String), ()> =
        report.updated.iter().cloned().map(|k| (k, ())).collect();
    let removed: HashMap<(String, String), ()> =
        report.removed.iter().cloned().map(|k| (k, ())).collect();

    // Read + parse the existing user file (if present). A missing file is an
    // empty patch; a malformed file is a hard error here (unlike `load`, which
    // falls back — the refresh tool should not silently drop user rows).
    let existing: zoid_model::RegistryPatch = match std::fs::read_to_string(user_path) {
        Ok(text) => crate::parse::parse_user(&text)
            .with_context(|| format!("parsing existing user file {}", user_path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            zoid_model::RegistryPatch::default()
        }
        Err(e) => return Err(anyhow::anyhow!("read {}: {e}", user_path.display())),
    };

    // Build the output patch, provider by provider. We preserve the existing
    // file's provider order, then append any provider that only appears in the
    // report (a brand-new provider getting its first wire row).
    let mut out: Vec<RawProviderSer> = Vec::new();
    let mut seen_providers: std::collections::HashSet<String> = std::collections::HashSet::new();

    for ep in &existing.providers {
        seen_providers.insert(ep.id.clone());
        let mut rows: Vec<RawModelSer> = Vec::new();
        for em in &ep.models {
            let key = (ep.id.clone(), em.id.clone());
            // Drop removed wire rows.
            if removed.contains_key(&key) {
                continue;
            }
            // Rewrite updated wire rows with the refreshed caps.
            if let Some(()) = updated.get(&key) {
                if let Some(info) = report.caps.get(&key) {
                    let wire_shape = wire_shape_for(reg, &ep.id, &em.id);
                    rows.push(RawModelSer::wire_row(&em.id, wire_shape, info));
                    continue;
                }
                // No caps in the report (fetch returned None) — keep the
                // existing row verbatim (couldn't refresh, spec §7).
            }
            // Otherwise carry the row through verbatim (user rows, untouched
            // wire rows, etc.).
            rows.push(RawModelSer::from_patch(em));
        }
        out.push(RawProviderSer {
            id: ep.id.clone(),
            model: rows,
        });
    }

    // Append the added wire rows. Group by provider; a provider that has no
    // existing entry gets a new one (appended in report order).
    for (pid, mid) in &report.added {
        let key = (pid.clone(), mid.clone());
        let info = report
            .caps
            .get(&key)
            .copied()
            .unwrap_or(zoid_model::DEFAULT_MODEL_INFO);
        let wire_shape = wire_shape_for(reg, pid, mid);
        let row = RawModelSer::wire_row(mid, wire_shape, &info);
        match out.iter_mut().find(|p| &p.id == pid) {
            Some(p) => p.model.push(row),
            None => {
                seen_providers.insert(pid.clone());
                out.push(RawProviderSer {
                    id: pid.clone(),
                    model: vec![row],
                });
            }
        }
    }

    let ser = RawRegistrySer { provider: out };
    let text = toml::to_string_pretty(&ser)
        .context("serializing refreshed user registry to TOML")?;

    // Atomic write: temp file in the same directory, then rename.
    let dir = user_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        user_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("models.user.toml")
    ));
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, user_path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), user_path.display()))?;
    Ok(())
}

/// Look up the `wire_shape` for a (provider, model) pair in the merged
/// registry, falling back to `OpenAIChat` when the model isn't present yet
/// (the common case for a brand-new `added` row on a report-only provider —
/// though in practice `added` only fires for wire-capable providers, which
/// always have a curated shape in the shipped file).
fn wire_shape_for(reg: &Registry, provider: &str, model: &str) -> WireShape {
    reg.wire_shape(provider, model).unwrap_or(WireShape::OpenAIChat)
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

    #[test]
    fn write_user_file_round_trips_wire_rows() {
        // Wire rows live in the user file (the shipped file only allows
        // `source = "static"`). So the existing user file starts with two
        // wire rows ("old-wire", "upd-wire") plus one `user` row ("user-keep"
        // that must be preserved verbatim). The shipped registry has only the
        // static "keep-static" row. The merged registry (= shipped + user)
        // is what `reconcile` ran against and what we pass to `write_user_file`.
        let shipped = Registry {
            providers: vec![http_provider(
                "ollama-cloud",
                vec![static_model("keep-static")],
            )],
        };
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("models.user.toml");
        std::fs::write(
            &user_path,
            r#"[[provider]]
id = "ollama-cloud"

  [[provider.model]]
  id = "user-keep"
  source = "user"
  hidden = true

  [[provider.model]]
  id = "old-wire"
  source = "wire"
  wire_shape = "ollama"
  context_window = 8000

  [[provider.model]]
  id = "upd-wire"
  source = "wire"
  wire_shape = "ollama"
  context_window = 8000
"#,
        )
        .unwrap();

        // The merged registry the refresh tool loaded (shipped + user file).
        let user_patch = crate::parse::parse_user(&std::fs::read_to_string(&user_path).unwrap())
            .unwrap();
        let reg = crate::merge::merge(
            shipped.clone(),
            user_patch,
        );

        // Report: add new-wire (with caps), update upd-wire (new caps), remove
        // old-wire. keep-static and user-keep are untouched.
        let mut report = ReconcileReport::default();
        report.added.push(("ollama-cloud".to_string(), "new-wire".to_string()));
        report.caps.insert(
            ("ollama-cloud".to_string(), "new-wire".to_string()),
            ModelInfo { context_window: 131_072, max_output: 0, tools: true, prompt_cache: true, ..info() },
        );
        report.updated.push(("ollama-cloud".to_string(), "upd-wire".to_string()));
        report.caps.insert(
            ("ollama-cloud".to_string(), "upd-wire".to_string()),
            ModelInfo { context_window: 65_536, ..info() },
        );
        report.removed.push(("ollama-cloud".to_string(), "old-wire".to_string()));

        write_user_file(&user_path, &reg, &report).unwrap();

        // Round-trip: parse the written file back and merge it over the
        // shipped registry (which only has the static row).
        let written = std::fs::read_to_string(&user_path).unwrap();
        let patch = crate::parse::parse_user(&written).unwrap();
        let merged = crate::merge::merge(shipped, patch);

        let p = merged.entry("ollama-cloud").unwrap();
        // user-keep preserved (hidden user row).
        let uk = p.models.iter().find(|m| m.id == "user-keep").unwrap();
        assert_eq!(uk.source, Source::User);
        assert!(uk.hidden);
        // old-wire removed.
        assert!(p.models.iter().all(|m| m.id != "old-wire"));
        // new-wire added with the fetched caps.
        let nw = p.models.iter().find(|m| m.id == "new-wire").unwrap();
        assert_eq!(nw.source, Source::Wire);
        assert_eq!(nw.info.context_window, 131_072);
        assert!(nw.info.tools);
        // upd-wire updated to the new caps.
        let uw = p.models.iter().find(|m| m.id == "upd-wire").unwrap();
        assert_eq!(uw.source, Source::Wire);
        assert_eq!(uw.info.context_window, 65_536);
        // keep-static untouched.
        assert!(p.models.iter().any(|m| m.id == "keep-static" && m.source == Source::Static));
    }

    #[test]
    fn write_user_file_handles_missing_user_file() {
        // No existing user file: the writer should create one with just the
        // added wire rows.
        let reg = Registry {
            providers: vec![http_provider("ollama-cloud", vec![])],
        };
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("models.user.toml");

        let mut report = ReconcileReport::default();
        report.added.push(("ollama-cloud".to_string(), "solo-wire".to_string()));
        report.caps.insert(
            ("ollama-cloud".to_string(), "solo-wire".to_string()),
            ModelInfo { context_window: 4096, ..info() },
        );

        write_user_file(&user_path, &reg, &report).unwrap();
        assert!(user_path.exists(), "user file must be created when absent");

        let written = std::fs::read_to_string(&user_path).unwrap();
        let patch = crate::parse::parse_user(&written).unwrap();
        assert_eq!(patch.providers.len(), 1);
        assert_eq!(patch.providers[0].id, "ollama-cloud");
        assert_eq!(patch.providers[0].models.len(), 1);
        assert_eq!(patch.providers[0].models[0].id, "solo-wire");
        assert_eq!(patch.providers[0].models[0].source, Some(Source::Wire));
        assert_eq!(patch.providers[0].models[0].context_window, Some(4096));
    }

    #[test]
    fn write_user_file_uses_defaults_when_caps_absent() {
        // An added wire row whose caps are NOT in report.caps (the fetch
        // returned None) must still be written, with conservative defaults.
        let reg = Registry {
            providers: vec![http_provider("ollama-cloud", vec![])],
        };
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("models.user.toml");

        let mut report = ReconcileReport::default();
        // Added with NO caps entry — reconcile reports this case via `reported`.
        report.added.push(("ollama-cloud".to_string(), "no-caps".to_string()));

        write_user_file(&user_path, &reg, &report).unwrap();
        let written = std::fs::read_to_string(&user_path).unwrap();
        let patch = crate::parse::parse_user(&written).unwrap();
        let m = &patch.providers[0].models[0];
        assert_eq!(m.id, "no-caps");
        assert_eq!(m.source, Some(Source::Wire));
        assert_eq!(m.context_window, Some(zoid_model::DEFAULT_MODEL_INFO.context_window));
    }
}