//! Pure source-resolution decision for `:plugin install <arg>`. The bin performs
//! the actual fetch and passes the observed facts (does the repo carry a
//! manifest? is there a bundled manifest for this URL?) into `resolve_source`,
//! keeping this module IO-free and table-testable.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRef {
    Id(String),
    Url(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestSource {
    Bundled,
    Repo,
    WizardFallback,
    Catalog,
}

/// A bare token is an id; anything that looks like a github URL is a Url.
pub fn classify_ref(arg: &str) -> PluginRef {
    let a = arg.trim();
    let looks_url = a.starts_with("github.com/")
        || a.starts_with("http://github.com/")
        || a.starts_with("https://github.com/");
    if looks_url {
        PluginRef::Url(a.to_string())
    } else {
        PluginRef::Id(a.to_string())
    }
}

/// Decide which manifest source to use. For an `Id`: bundled if known, else
/// `WizardFallback` (caller reports "unknown plugin"). For a `Url`: repo
/// manifest wins, then a bundled manifest keyed to that URL, then the
/// model-driven wizard.
pub fn resolve_source(
    r: &PluginRef,
    bundled_ids: &[&str],
    repo_has_manifest: bool,
    bundled_for_url: bool,
) -> ManifestSource {
    match r {
        PluginRef::Id(id) => {
            if bundled_ids.contains(&id.as_str()) {
                ManifestSource::Bundled
            } else {
                ManifestSource::Catalog
            }
        }
        PluginRef::Url(_) => {
            if repo_has_manifest {
                ManifestSource::Repo
            } else if bundled_for_url {
                ManifestSource::Bundled
            } else {
                ManifestSource::WizardFallback
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ref_distinguishes_url_from_id() {
        assert_eq!(classify_ref("superpowers"), PluginRef::Id("superpowers".into()));
        assert_eq!(
            classify_ref("github.com/obra/superpowers/tree/main/skills"),
            PluginRef::Url("github.com/obra/superpowers/tree/main/skills".into())
        );
        assert!(matches!(
            classify_ref("https://github.com/o/r/tree/main/x"),
            PluginRef::Url(_)
        ));
    }

    #[test]
    fn id_resolves_to_bundled_when_known() {
        let r = PluginRef::Id("superpowers".into());
        assert_eq!(
            resolve_source(&r, &["superpowers"], false, false),
            ManifestSource::Bundled
        );
    }

    #[test]
    fn unknown_id_resolves_to_catalog() {
        let r = PluginRef::Id("ok-skills".into());
        assert_eq!(resolve_source(&r, &["superpowers"], false, false), ManifestSource::Catalog);
    }

    #[test]
    fn known_id_still_bundled() {
        let r = PluginRef::Id("superpowers".into());
        assert_eq!(resolve_source(&r, &["superpowers"], false, false), ManifestSource::Bundled);
    }

    #[test]
    fn url_prefers_repo_manifest_then_bundled_then_wizard() {
        let r = PluginRef::Url("github.com/o/r/tree/main/skills".into());
        assert_eq!(resolve_source(&r, &[], true, false), ManifestSource::Repo);
        assert_eq!(resolve_source(&r, &[], false, true), ManifestSource::Bundled);
        assert_eq!(
            resolve_source(&r, &[], false, false),
            ManifestSource::WizardFallback
        );
    }
}
