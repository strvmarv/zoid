use zoid_plugin_import::{
    classify::{classify, KindPref, PluginTree},
    claude,
    emit::{self, emit},
    fetch,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("repo") => run_repo(&args[1..]).await,
        Some("bulk") => run_bulk(&args[1..]).await,
        _ => {
            eprintln!("usage: zoid-plugin-import <repo|bulk> ...");
            std::process::exit(2);
        }
    }
}

fn parse_pref(args: &[String]) -> KindPref {
    if args.iter().any(|a| a == "--mode") {
        KindPref::Mode
    } else if args.iter().any(|a| a == "--skills") {
        KindPref::Skills
    } else {
        KindPref::Auto
    }
}

async fn run_repo(args: &[String]) -> anyhow::Result<()> {
    let spec = args
        .first()
        .anyhow_context("missing <owner/name[/subpath]>")?;
    let pref = parse_pref(args);
    // Split owner/name[/subpath]; subtree defaults to "skills".
    let mut parts = spec.splitn(3, '/');
    let owner = parts.next().unwrap();
    let name = parts.next().anyhow_context("expected owner/name")?;
    let repo = format!("{owner}/{name}");
    let subtree = parts
        .next()
        .unwrap_or("skills")
        .trim_end_matches('/')
        .to_string();
    let sha = fetch::resolve_head_sha(&repo, "HEAD")?;
    let files = fetch::fetch_tree_paths(&repo, &sha).await?;
    let mcp_json = if files.iter().any(|f| f == ".mcp.json") {
        Some(fetch::fetch_blob(&repo, &sha, ".mcp.json").await?)
    } else {
        None
    };
    // For classification we need skill file paths RELATIVE to the plugin root;
    // for a repo-root plugin they already are. Build the tree.
    let plugin = claude::PluginJson {
        name: name.to_string(),
        description: String::new(),
    };
    let tree = PluginTree {
        files,
        mcp_json: mcp_json.clone(),
        plugin_json: plugin,
    };
    let c = classify(&tree, pref);
    let e = emit(name, "", &repo, &sha, &subtree, &c, mcp_json.as_deref())?;
    print_emitted(&repo, &e);
    Ok(())
}

async fn run_bulk(args: &[String]) -> anyhow::Result<()> {
    let path = args.first().anyhow_context("missing <marketplace.json>")?;
    let entries = claude::parse_marketplace(&std::fs::read_to_string(path)?)?;
    for entry in entries {
        // Resolve repo+sha from the source ref (pinned in the marketplace).
        let (repo, sha, subtree) = match &entry.source {
            claude::PluginSourceRef::GitSubdir { url, path, sha } => (
                url.trim_end_matches(".git")
                    .trim_start_matches("https://github.com/")
                    .to_string(),
                sha.clone(),
                format!("{}/skills", path.trim_end_matches('/')),
            ),
            claude::PluginSourceRef::Github { repo, sha } => {
                (repo.clone(), sha.clone(), "skills".into())
            }
            claude::PluginSourceRef::InRepo { .. } => {
                eprintln!(
                    "skip in-repo {} (bulk needs the marketplace repo sha)",
                    entry.name
                );
                continue;
            }
        };
        let files = match fetch::fetch_tree_paths(&repo, &sha).await {
            Ok(f) => f,
            Err(e) => {
                eprintln!("skip {}: {e}", entry.name);
                continue;
            }
        };
        let mcp_json = if files.iter().any(|f| f.ends_with(".mcp.json")) {
            fetch::fetch_blob(&repo, &sha, ".mcp.json").await.ok()
        } else {
            None
        };
        let tree = PluginTree {
            files,
            mcp_json: mcp_json.clone(),
            plugin_json: claude::PluginJson {
                name: entry.name.clone(),
                description: entry.description.clone(),
            },
        };
        let c = classify(&tree, KindPref::Auto);
        match emit(
            &entry.name,
            &entry.description,
            &repo,
            &sha,
            subtree.trim_end_matches("/skills"),
            &c,
            mcp_json.as_deref(),
        ) {
            Ok(e) => print_emitted(&repo, &e),
            Err(e) => eprintln!("emit {}: {e}", entry.name),
        }
    }
    Ok(())
}

fn print_emitted(repo: &str, e: &emit::Emitted) {
    println!("== {repo} ==\n{}", e.report);
    if let Some(t) = &e.plugin_toml {
        println!("--- plugin.toml ---\n{t}");
    }
    if let Some(m) = &e.mcp_json {
        println!("--- .mcp.json ---\n{m}");
    }
}

trait AnyhowContext<T> {
    fn anyhow_context(self, msg: &str) -> anyhow::Result<T>;
}
impl<T> AnyhowContext<T> for Option<T> {
    fn anyhow_context(self, msg: &str) -> anyhow::Result<T> {
        self.ok_or_else(|| anyhow::anyhow!(msg.to_string()))
    }
}
