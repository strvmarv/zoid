//! The `.zoid/plugin.toml` manifest schema (schema = 1) + parse + validate.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::effect::Effect;

#[derive(Debug, Clone, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub schema: u32,
    pub kind: Vec<String>,
    pub name: String,
    pub description: String,
    pub source: Option<PluginSource>,
    pub mode: Option<ModeRecipe>,
    pub mcp: Option<McpManifest>,
    pub install: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginSource {
    pub repo: String,
    pub ref_: String,
    pub subtree: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeRecipe {
    pub loader: String,
    pub strip_prefix: String,
    pub body: BodyStrategy,
    pub description: String,
    pub body_intro: Option<String>,
    pub body_outro: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BodyStrategy {
    FromSkillFrontmatter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpManifest {
    pub servers: BTreeMap<String, McpServerSpec>,
}

// --- Raw serde shapes (mirror the TOML layout), converted into the public
// types above so the public API isn't coupled to serde field naming. ---

#[derive(Deserialize)]
struct RawManifest {
    plugin: RawPlugin,
    source: Option<RawSource>,
    mode: Option<RawMode>,
    mcp: Option<RawMcp>,
    #[serde(default)]
    install: Vec<RawEffect>,
}

#[derive(Deserialize)]
struct RawPlugin {
    id: String,
    schema: u32,
    kind: Vec<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize)]
struct RawSource {
    repo: String,
    #[serde(rename = "ref")]
    ref_: String,
    subtree: String,
}

#[derive(Deserialize)]
struct RawMode {
    loader: String,
    #[serde(default)]
    strip_prefix: String,
    body: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body_intro: Option<String>,
    #[serde(default)]
    body_outro: Option<String>,
}

#[derive(Deserialize)]
struct RawMcp {
    #[serde(default)]
    servers: BTreeMap<String, RawMcpServer>,
}

#[derive(Deserialize)]
struct RawMcpServer {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RawEffect {
    effect: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<toml::Value>,
}

/// Parse a manifest from TOML source. Unknown *keys* are ignored by serde
/// (forward-compat, mirroring config.toml's warn-not-reject stance); unknown
/// *effect names* are a hard error (an unrecognized effect must never be
/// silently dropped).
pub fn parse_manifest(toml_src: &str) -> Result<PluginManifest, String> {
    let raw: RawManifest =
        toml::from_str(toml_src).map_err(|e| format!("plugin.toml parse error: {e}"))?;

    let body = match raw.mode.as_ref().map(|m| m.body.as_str()) {
        None => None,
        Some("from-skill-frontmatter") => Some(BodyStrategy::FromSkillFrontmatter),
        Some(other) => return Err(format!("unknown mode body strategy '{other}'")),
    };

    let mut install = Vec::new();
    for e in raw.install {
        let effect = match e.effect.as_str() {
            "activate" => Effect::Activate,
            "onboarding_hint" => Effect::OnboardingHint {
                text: e.text.unwrap_or_default(),
            },
            "set_config" => Effect::SetConfig {
                key: e
                    .key
                    .ok_or_else(|| "set_config effect missing 'key'".to_string())?,
                value: e
                    .value
                    .ok_or_else(|| "set_config effect missing 'value'".to_string())?,
            },
            other => return Err(format!("unknown install effect '{other}'")),
        };
        install.push(effect);
    }

    Ok(PluginManifest {
        id: raw.plugin.id,
        schema: raw.plugin.schema,
        kind: raw.plugin.kind,
        name: raw.plugin.name,
        description: raw.plugin.description,
        source: raw.source.map(|s| PluginSource {
            repo: s.repo,
            ref_: s.ref_,
            subtree: s.subtree,
        }),
        mode: raw.mode.map(|m| ModeRecipe {
            loader: m.loader,
            strip_prefix: m.strip_prefix,
            body: body.expect("body set when mode present"),
            description: m.description,
            body_intro: m.body_intro,
            body_outro: m.body_outro,
        }),
        mcp: raw.mcp.map(|m| McpManifest {
            servers: m
                .servers
                .into_iter()
                .map(|(name, s)| {
                    (
                        name,
                        McpServerSpec {
                            command: s.command,
                            args: s.args,
                            env: s.env,
                        },
                    )
                })
                .collect(),
        }),
        install,
    })
}

impl PluginManifest {
    /// Validate that this manifest is installable by *this* zoid version.
    /// Unknown artifact kinds and a `mode` kind without a `[mode]` table are
    /// rejected here (typed seams: future kinds fail cleanly, never silently).
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!(
                "plugin '{}' declares schema {} (this zoid supports schema 1)",
                self.id, self.schema
            ));
        }
        let is_mcp = self.kind.iter().any(|k| k == "mcp");
        if is_mcp {
            // mcp is not composable with the tree-materializing kinds; the
            // install dispatch can only route one way.
            if self.kind != ["mcp"] {
                return Err(format!(
                    "plugin '{}' mixes 'mcp' with other kinds; 'mcp' must be the only kind",
                    self.id
                ));
            }
            if self.source.is_some() || self.mode.is_some() {
                return Err(format!(
                    "plugin '{}' is kind 'mcp' and must not declare [source] or [mode]",
                    self.id
                ));
            }
            match self.mcp.as_ref().map(|m| m.servers.len()) {
                Some(1) => {}
                _ => {
                    return Err(format!(
                        "plugin '{}' (kind 'mcp') must declare exactly one server",
                        self.id
                    ));
                }
            }
            if let Some(m) = self.mcp.as_ref() {
                for (name, s) in &m.servers {
                    if s.command.trim().is_empty() {
                        return Err(format!(
                            "plugin '{}' (kind 'mcp') server '{}' has an empty command",
                            self.id, name
                        ));
                    }
                }
            }
            return Ok(());
        }
        for k in &self.kind {
            if k != "mode" && k != "skills" {
                return Err(format!(
                    "plugin '{}' declares unsupported kind '{}' (v1 supports 'mode', 'skills', 'mcp')",
                    self.id, k
                ));
            }
        }
        if self.kind.iter().any(|k| k == "mode") && self.mode.is_none() {
            return Err(format!(
                "plugin '{}' declares kind 'mode' but has no [mode] table",
                self.id
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[plugin]
id = "superpowers"
schema = 1
kind = ["mode"]
name = "Superpowers"
description = "Skill-driven workflows"

[source]
repo = "obra/superpowers"
ref = "d884ae04"
subtree = "skills"

[mode]
loader = "using-superpowers/SKILL.md"
strip_prefix = "skills/"
body = "from-skill-frontmatter"
description = "Superpowers — curated skills"

[[install]]
effect = "activate"

[[install]]
effect = "onboarding_hint"
text = "Superpowers installed."
"#;

    #[test]
    fn parses_a_good_manifest() {
        let m = parse_manifest(GOOD).unwrap();
        assert_eq!(m.id, "superpowers");
        assert_eq!(m.kind, vec!["mode".to_string()]);
        assert_eq!(m.source.as_ref().unwrap().ref_, "d884ae04");
        let mode = m.mode.as_ref().unwrap();
        assert_eq!(mode.loader, "using-superpowers/SKILL.md");
        assert_eq!(mode.strip_prefix, "skills/");
        assert!(matches!(mode.body, BodyStrategy::FromSkillFrontmatter));
        assert_eq!(m.install.len(), 2);
        assert_eq!(m.install[0], Effect::Activate);
        assert_eq!(
            m.install[1],
            Effect::OnboardingHint {
                text: "Superpowers installed.".into()
            }
        );
        m.validate().unwrap();
    }

    #[test]
    fn rejects_unknown_kind() {
        let src = GOOD.replace(r#"kind = ["mode"]"#, r#"kind = ["mode", "wormhole"]"#);
        let m = parse_manifest(&src).unwrap();
        let err = m.validate().unwrap_err();
        assert!(err.contains("wormhole"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_effect() {
        let src = GOOD.replace(r#"effect = "activate""#, r#"effect = "rm_rf""#);
        let err = parse_manifest(&src).unwrap_err();
        assert!(
            err.contains("rm_rf") || err.contains("effect"),
            "got: {err}"
        );
    }

    #[test]
    fn mode_kind_requires_a_mode_table() {
        let src = GOOD.replace("[mode]", "[unused]");
        let m = parse_manifest(&src).unwrap();
        let err = m.validate().unwrap_err();
        assert!(err.contains("mode"), "got: {err}");
    }

    #[test]
    fn parses_mode_body_intro_outro() {
        let src = GOOD.replace(
            "body = \"from-skill-frontmatter\"",
            "body = \"from-skill-frontmatter\"\nbody_intro = \"INTRO\"\nbody_outro = \"OUTRO\"",
        );
        let m = parse_manifest(&src).unwrap();
        let mode = m.mode.as_ref().unwrap();
        assert_eq!(mode.body_intro.as_deref(), Some("INTRO"));
        assert_eq!(mode.body_outro.as_deref(), Some("OUTRO"));
    }

    #[test]
    fn mode_body_intro_outro_default_to_none() {
        let m = parse_manifest(GOOD).unwrap();
        let mode = m.mode.as_ref().unwrap();
        assert!(mode.body_intro.is_none());
        assert!(mode.body_outro.is_none());
    }

    #[test]
    fn accepts_skills_kind_without_mode_table() {
        let src = r#"
[plugin]
id = "doctools"
schema = 1
kind = ["skills"]
name = "Doc Tools"
description = "on-demand skills"

[source]
repo = "anthropics/skills"
ref = "SHA"
subtree = "skills"
"#;
        let m = parse_manifest(src).unwrap();
        m.validate().unwrap();
        assert_eq!(m.kind, vec!["skills".to_string()]);
        assert!(m.mode.is_none());
    }

    const MCP_GOOD: &str = r#"
[plugin]
id = "github"
schema = 1
kind = ["mcp"]
name = "GitHub MCP"
description = "GitHub over MCP"

[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "${GITHUB_TOKEN}" }
"#;

    #[test]
    fn parses_and_validates_an_mcp_manifest() {
        let m = parse_manifest(MCP_GOOD).unwrap();
        m.validate().unwrap();
        assert_eq!(m.kind, vec!["mcp".to_string()]);
        assert!(m.source.is_none() && m.mode.is_none());
        let mcp = m.mcp.as_ref().unwrap();
        assert_eq!(mcp.servers.len(), 1);
        let s = mcp.servers.get("github").unwrap();
        assert_eq!(s.command, "npx");
        assert_eq!(s.args, vec!["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(s.env.get("GITHUB_TOKEN").unwrap(), "${GITHUB_TOKEN}");
    }

    #[test]
    fn rejects_mcp_mixed_with_other_kinds() {
        let src = MCP_GOOD.replace(r#"kind = ["mcp"]"#, r#"kind = ["mcp", "skills"]"#);
        let m = parse_manifest(&src).unwrap();
        assert!(m.validate().unwrap_err().contains("mcp"));
    }

    #[test]
    fn rejects_mcp_without_a_server() {
        let src = "\n[plugin]\nid = \"x\"\nschema = 1\nkind = [\"mcp\"]\nname = \"X\"\ndescription = \"d\"\n";
        let m = parse_manifest(src).unwrap();
        assert!(m.validate().unwrap_err().contains("server"));
    }

    #[test]
    fn rejects_mcp_with_more_than_one_server() {
        let src = format!("{MCP_GOOD}\n[mcp.servers.second]\ncommand = \"foo\"\n");
        let m = parse_manifest(&src).unwrap();
        assert!(m.validate().unwrap_err().contains("one server"));
    }

    #[test]
    fn rejects_mcp_with_source_or_mode() {
        let src = format!("{MCP_GOOD}\n[source]\nrepo = \"a/b\"\nref = \"s\"\nsubtree = \"x\"\n");
        let m = parse_manifest(&src).unwrap();
        assert!(m.validate().unwrap_err().contains("source"));
    }

    #[test]
    fn rejects_mcp_server_missing_command() {
        // `command` is required by the RawMcpServer serde shape → parse error.
        let src = "\n[plugin]\nid=\"x\"\nschema=1\nkind=[\"mcp\"]\nname=\"X\"\ndescription=\"d\"\n[mcp.servers.s]\nargs=[\"a\"]\n";
        assert!(parse_manifest(src).is_err());
    }

    #[test]
    fn rejects_mcp_server_with_empty_command() {
        let src = MCP_GOOD.replace(r#"command = "npx""#, r#"command = """#);
        let m = parse_manifest(&src).unwrap();
        assert!(m.validate().unwrap_err().contains("command"));
    }
}
