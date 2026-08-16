//! Skills the agent loop pulls on demand via the `invoke_skill` tool. A skill is
//! a named unit of instructions whose body is returned to the model as a tool
//! result — mirroring Claude Code's Skill tool. v1 ships two hand-written
//! built-in skills that chain (spike-plan → spike-implement) to prove the
//! runtime; the SKILL.md importer is a later slice. Pure: no provider/process deps.

/// A single named skill: its one-line menu description, its full body, and the
/// source directory it was imported from (for bundled sibling files). Built-in
/// skills have `base_dir: None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub base_dir: Option<std::path::PathBuf>,
}

/// The `name`/`description`/`body` extracted from a `SKILL.md`. Carries no
/// filesystem location — the caller (the bin's importer) assigns `base_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSkill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Strip one matching pair of surrounding single or double quotes.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let n = s.len();
    if n >= 2 && ((b[0] == b'"' && b[n - 1] == b'"') || (b[0] == b'\'' && b[n - 1] == b'\'')) {
        s[1..n - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parse a `SKILL.md` document: a `---`-fenced frontmatter block followed by the
/// markdown body. Reads the `name` and `description` scalar lines from the
/// frontmatter (stripping one matching pair of surrounding quotes); the body is
/// everything after the FIRST closing fence, verbatim. Pure — no filesystem.
/// Single-line scalar values only (YAML block scalars are out of scope).
///
/// Returns `Err` with a human-readable reason if there is no frontmatter block
/// or the `name` field is missing/empty.
pub fn parse_skill_md(text: &str) -> Result<ParsedSkill, String> {
    let after_open = text
        .strip_prefix("---")
        .ok_or("missing frontmatter opening '---'")?;
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close = after_open
        .find("\n---")
        .ok_or("missing frontmatter closing '---'")?;
    let front = &after_open[..close];
    // Everything from the closing "\n---" onward: drop the newline, the "---",
    // and one trailing newline to reach the body start.
    let rest = &after_open[close + 1..]; // starts at "---"
    let body = rest
        .strip_prefix("---")
        .map(|b| b.strip_prefix('\n').unwrap_or(b))
        .unwrap_or(rest)
        .to_string();

    let mut name = String::new();
    let mut description = String::new();
    for line in front.lines() {
        if let Some(v) = line.strip_prefix("name:") {
            name = unquote(v.trim());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = unquote(v.trim());
        }
    }
    if name.is_empty() {
        return Err("frontmatter is missing a non-empty 'name'".into());
    }
    Ok(ParsedSkill {
        name,
        description,
        body,
    })
}

/// The skills available to the current session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillRegistry {
    skills: Vec<Skill>,
}

/// The body of the built-in `feedback` skill. References the `submit_feedback`
/// tool and the `strvmarv/zoid` repo.
const FEEDBACK_SKILL_BODY: &str = "\
# Submitting Feedback & Bug Reports

zoid can file feedback or bug reports to the maintainers as GitHub issues on
`strvmarv/zoid`. The `submit_feedback` tool proposes a report; the
user **always confirms and can edit** before it is submitted — never file
silently.

## When to Offer

Offer the tool when:
- The user explicitly asks to \"report a bug\", \"give feedback\", or \"file an issue\".
- A reproducible error occurs AND the user agrees to report it (ask first via
  `ask_user` — don't assume).
- The user expresses frustration about zoid's behavior and a concrete, actionable
  issue can be identified.

Do NOT offer when:
- The user is frustrated with *their own code* (that's not a zoid bug).
- The error is clearly user error (wrong path, bad config) — help them instead.
- The user just wants to vent; only file if there's something actionable.

## Writing a Good Report

Call `submit_feedback` with a well-structured report:

- **kind**: `bug`, `feature`, or `general`.
- **title**: One line, specific. Bad: \"it crashed\". Good: \"Crash on `:config`
  open when no provider is configured\".
- **body**: For bugs — steps to reproduce, expected behavior, actual behavior.
  For features — the use case and the proposed solution. For general — what's
  on your mind.

Diagnostics (version, OS, session, mode, model, cwd, recent error) are
attached automatically — you don't need to gather them. But **describe the
user's situation in the body**, since you know the context that led here.

## After Submitting

The tool result tells you the outcome:
- `Created issue #N: <url>` — tell the user the issue number and URL.
- `Opened browser at <url>` — tell the user to finish submitting in the
  browser (no token was available), and give them the URL.
- `User declined` — acknowledge and move on; don't push.

Never call `submit_feedback` twice for the same issue in one session unless the
user asks.
";

/// The body of the built-in `refreshing-provider-models` skill. Guides an
/// agent to refresh the static provider/model registry in `zoid-model` against
/// live provider endpoints, add MODEL_CAPS entries for new models, and verify.
const REFRESHING_PROVIDER_MODELS_BODY: &str = concat!(
    "# Refreshing Provider Models\n\n",
    "Refresh the static provider/model registry in `crates/zoid-model/src/lib.rs`\n",
    "against live provider endpoints. Three targets: `PROVIDERS` model id arrays,\n",
    "`ZEN_MODEL_IDS`, and `MODEL_CAPS` (per-model capabilities).\n\n",
    "## Phase 1 — Fetch live model lists\n\n",
    "Run a `curl` GET per provider. Skip providers whose key is missing.\n\n",
    "| Provider id | Secret env var | Endpoint | Auth | Response path | Registry field |\n",
    "|---|---|---|---|---|---|\n",
    "| `ollama-local` | (keyless) | `{base}/api/tags` | Bearer (opt) | `.models[].name` | skip (free-text) |\n",
    "| `ollama-cloud` | `OLLAMA_API_KEY` | `https://ollama.com/api/tags` | Bearer | `.models[].name` | `ollama-cloud` models (curated) |\n",
    "| `opencode-go` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/go/v1/models` | Bearer | `.data[].id` | `opencode-go` models |\n",
    "| `opencode-zen` | `OPENCODE_GO_API_KEY` | `https://opencode.ai/zen/v1/models` | Bearer | `.data[].id` | `ZEN_MODEL_IDS` |\n",
    "| `anthropic-api` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1/models` | `x-api-key` + `anthropic-version: 2023-06-01` | `.data[].id` | `anthropic-api` models |\n",
    "| `zai-coding-plan` | `ZAI_API_KEY` | `https://api.z.ai/api/coding/paas/v4/models` | Bearer | `.data[].id` | `zai-coding-plan` models |\n\n",
    "**Critical:** `ollama-local` and `ollama-cloud` share `OllamaProvider` — both\n",
    "hit `/api/tags` and parse `.models[].name`. Neither is OpenAI-compat. Do not\n",
    "use `/v1/models` or `.data[].id` for either Ollama flavor.\n\n",
    "```bash\n",
    "# ollama-cloud (native Ollama API, not OpenAI-compat)\n",
    "curl -s -H \"Authorization: Bearer $OLLAMA_API_KEY\" https://ollama.com/api/tags | jq -r '.models[].name'\n",
    "# anthropic-api (NOT Bearer — uses x-api-key)\n",
    "curl -s -H \"x-api-key: $ANTHROPIC_API_KEY\" -H \"anthropic-version: 2023-06-01\" https://api.anthropic.com/v1/models | jq -r '.data[].id'\n",
    "# opencode-zen\n",
    "curl -s -H \"Authorization: Bearer $OPENCODE_GO_API_KEY\" https://opencode.ai/zen/v1/models | jq -r '.data[].id'\n",
    "```\n\n",
    "## Phase 2 — Diff and update\n\n",
    "### 2a. Model id lists\n\n",
    "- Add ids present live but missing. Remove ids absent live (retired).\n",
    "- Preserve `PROVIDERS` order — picker display order (convention). Insert new\n",
    "  ids grouped with siblings.\n",
    "- `ollama-local` stays `&[]` — never populate it.\n",
    "- `ollama-cloud` is **curated** (`&[\"glm-5.2:cloud\"]`), not a live-list\n",
    "  mirror. Preserve the `:cloud` suffix; new cloud ids need MODEL_CAPS entries.\n",
    "- `ZEN_MODEL_IDS` first entry is the default model — a **product decision**,\n",
    "  not endpoint-derivable. Do not change without explicit instruction. The\n",
    "  `// All NN Zen model ids` count comment (currently 52: 13 Anthropic +\n",
    "  17 OpenAI Responses + 19 OpenAI Chat + 3 Gemini) must be updated to match.\n",
    "- Cross-array duplication is expected (`glm-5.2` appears in Zen, Go, ZAI).\n",
    "  Dedup matters only within `MODEL_CAPS` (case-insensitive), not across\n",
    "  provider id arrays.\n\n",
    "### 2b. MODEL_CAPS for new ids\n\n",
    "All unknowns fall back to `DEFAULT_MODEL_INFO` (`lib.rs:640`): 32k / 0 /\n",
    "tools=true / prompt_cache=false / None / None.\n\n",
    "**Exception:** `opencode_zen_model_caps_present` asserts every `opencode-zen`\n",
    "model has `context_window >= 128_000` — the 32k default is not acceptable for\n",
    "selectable Zen/Go models. New Zen/Go ids must have an explicit researched\n",
    "entry. (The `opencode_zen_caps_match_table` lock test has 39 cases — the 13\n",
    "that overlap with Go are excluded; it doesn't auto-catch *new* ids, but\n",
    "`opencode_zen_model_caps_present` does via the >=128k gate.)\n\n",
    "`ModelInfo` fields (see struct at `lib.rs:15`): `context_window` (u64),\n",
    "`max_output` (u64, 0 = provider default), `tools` (bool), `prompt_cache`\n",
    "(bool), `thinking` (ThinkingSupport), `thinking_wire` (ThinkingWireShape).\n\n",
    "**`thinking_wire` is per-model, not per-family.** Many Anthropic-routed Go/Zen\n",
    "models have `thinking_wire: None`. Copy from a researched sibling of the same\n",
    "family/variant where one exists; otherwise `None`.\n\n",
    "Do not duplicate `MODEL_CAPS` entries — lookup is case-insensitive, duplicates\n",
    "silently shadow.\n\n",
    "### 2c. Provider metadata\n\n",
    "Verify `default_base_url` still resolves (Phase 1 proved reachability). Verify\n",
    "`key_url` is still valid — `ollama-local` must be `None`, all others `Some(_)`\n",
    "(the test is keyed on provider id). Flag dark providers, do not remove without\n",
    "confirmation.\n\n",
    "## Phase 3 — Verify\n\n",
    "```bash\n",
    "cargo test -p zoid-model    # registry invariants\n",
    "cargo build -p zoid-provider # re-exports compile\n",
    "cargo test -p zoid-provider  # wire-shape routing tables\n",
    "```\n\n",
    "**Wire-shape routing tables:** Adding a new id to `ZEN_MODEL_IDS` requires a\n",
    "matching entry in `opencode_zen.rs::ZEN_MODELS`, or it silently defaults to\n",
    "`OpenAIChat` (wrong wire shape, no test failure). Likewise, new `opencode-go`\n",
    "ids need an entry in `opencode_go.rs::GO_MODELS`. These are in\n",
    "`crates/zoid-provider/src/`, separate from the registry's `models` arrays.\n\n",
    "Key test invariants:\n",
    "- `selectable_has_six_providers` — exactly six selectable providers.\n",
    "- `opencode_go_entry_unchanged` — Go has exactly 13 models (update the\n",
    "  literal if adding/removing Go ids).\n",
    "- `opencode_go_model_caps_match_reconciled_table` — locks all 13 Go caps.\n",
    "- `opencode_zen_model_caps_present` — every Zen model >= 128k context.\n",
    "- `key_url_field_present_on_all_providers` — ollama-local=None, rest=Some.\n",
    "- `model_info_unknown_falls_back_to_conservative_default` — unknown -> 32k.\n",
);

impl SkillRegistry {
    /// Build a registry from an explicit skill list.
    pub fn new(skills: Vec<Skill>) -> Self {
        Self { skills }
    }

    /// Append `skill` unless a skill with the same name already exists. Returns
    /// `true` if appended, `false` (and leaves the registry unchanged) on a name
    /// collision — first-wins, so built-ins and earlier imports are protected.
    pub fn push_unique(&mut self, skill: Skill) -> bool {
        if self.skills.iter().any(|s| s.name == skill.name) {
            return false;
        }
        self.skills.push(skill);
        true
    }

    /// The two hand-written built-in spike skills. `spike-plan` ends by
    /// instructing the model to invoke `spike-implement` — the chaining proof.
    /// Both bodies reference ONLY tools that exist in zoid (`invoke_skill`,
    /// `Write`).
    pub fn builtin() -> Self {
        Self::new(vec![
            Skill {
                name: "spike-plan".into(),
                description:
                    "Draft the plan for the spike task, then hand off to spike-implement.".into(),
                body: "You are executing the 'spike-plan' skill.\n\n\
                    The task: create a file at ./spike-artifact.txt whose only line is: spike ok\n\n\
                    Step 1: restate that plan in one short sentence.\n\
                    Step 2: to carry the plan out, call the invoke_skill tool with name \
                    \"spike-implement\".\n\
                    Do NOT write the file yourself in this step — spike-implement does that."
                    .into(),
                base_dir: None,
            },
            Skill {
                name: "spike-implement".into(),
                description: "Write the spike artifact file described by the plan.".into(),
                body: "You are executing the 'spike-implement' skill.\n\n\
                    Create the file ./spike-artifact.txt with exactly one line of content: spike ok\n\
                    Use the write tool. Then confirm in one sentence that you wrote it."
                    .into(),
                base_dir: None,
            },
            Skill {
                name: "feedback".into(),
                description: "Use when the user asks to report a bug or give feedback, \
                    or when a reproducible error occurs and the user agrees to report it — \
                    offers the submit_feedback tool to file a GitHub issue on \
                    strvmarv/zoid, with the user confirming before anything \
                    is submitted.".into(),
                body: FEEDBACK_SKILL_BODY.into(),
                base_dir: None,
            },
            Skill {
                name: "refreshing-provider-models".into(),
                description: "Use when refreshing zoid's static provider/model \
                    registry against live provider endpoints, adding new models \
                    to MODEL_CAPS, reconciling model id drift, or updating \
                    provider metadata across the six supported providers".into(),
                body: REFRESHING_PROVIDER_MODELS_BODY.into(),
                base_dir: None,
            },
        ])
    }

    /// Look up a skill by exact name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// All skill names in registry order.
    pub fn names(&self) -> Vec<String> {
        self.skills.iter().map(|s| s.name.clone()).collect()
    }

    /// All skills in registry order (for composing scoped views).
    pub fn all(&self) -> &[Skill] {
        &self.skills
    }

    /// The menu injected into a mode's system prompt: one `- name: description`
    /// line per skill, in registry order. Empty string when there are no skills.
    pub fn menu(&self) -> String {
        self.skills
            .iter()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_both_spike_skills_that_chain() {
        let r = SkillRegistry::builtin();
        assert_eq!(
            r.names(),
            vec![
                "spike-plan".to_string(),
                "spike-implement".to_string(),
                "feedback".to_string(),
                "refreshing-provider-models".to_string()
            ]
        );
        let plan = r.get("spike-plan").unwrap();
        assert!(
            plan.body.contains("spike-implement"),
            "spike-plan must chain to spike-implement"
        );
        assert!(plan.body.contains("invoke_skill"));
        let imp = r.get("spike-implement").unwrap();
        assert!(imp.body.contains("write"));
        assert!(imp.body.contains("spike-artifact.txt"));
    }

    #[test]
    fn get_misses_unknown_name() {
        assert!(SkillRegistry::builtin().get("nope").is_none());
    }

    #[test]
    fn menu_renders_one_line_per_skill() {
        let menu = SkillRegistry::builtin().menu();
        assert!(menu.contains("- spike-plan: "));
        assert!(menu.contains("- spike-implement: "));
        assert!(menu.contains("- feedback: "));
        assert!(menu.contains("- refreshing-provider-models: "));
        assert_eq!(menu.lines().count(), 4);
    }

    #[test]
    fn empty_registry_menu_is_empty_string() {
        assert_eq!(SkillRegistry::new(vec![]).menu(), "");
    }

    #[test]
    fn builtin_skills_have_no_base_dir() {
        let r = SkillRegistry::builtin();
        assert!(r.get("spike-plan").unwrap().base_dir.is_none());
        assert!(r.get("spike-implement").unwrap().base_dir.is_none());
        assert!(r.get("feedback").unwrap().base_dir.is_none());
    }

    #[test]
    fn builtin_includes_feedback_skill() {
        let r = SkillRegistry::builtin();
        assert!(r.get("feedback").is_some());
        let fb = r.get("feedback").unwrap();
        assert!(
            fb.body.contains("submit_feedback"),
            "feedback skill must reference the submit_feedback tool"
        );
        assert!(fb.body.contains("strvmarv/zoid"));
        assert!(fb.base_dir.is_none());
    }

    #[test]
    fn push_unique_protects_feedback_builtin_from_shadow() {
        let mut r = SkillRegistry::builtin();
        let shadow = Skill {
            name: "feedback".into(),
            description: "shadow".into(),
            body: "SHADOW".into(),
            base_dir: None,
        };
        assert!(
            !r.push_unique(shadow),
            "an import must not shadow the built-in feedback"
        );
        assert_eq!(r.get("feedback").unwrap().body, FEEDBACK_SKILL_BODY);
    }

    #[test]
    fn push_unique_appends_new_and_rejects_duplicate() {
        let mk = |n: &str| Skill {
            name: n.into(),
            description: "d".into(),
            body: "b".into(),
            base_dir: None,
        };
        let mut r = SkillRegistry::new(vec![]);
        assert!(r.push_unique(mk("a")));
        assert!(!r.push_unique(mk("a"))); // duplicate name rejected, no change
        assert!(r.push_unique(mk("b")));
        assert_eq!(r.names(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn parses_name_description_and_body() {
        let md = "---\nname: brainstorming\n\
                  description: \"Explore: intent, and design\"\n\
                  ---\n# Body\n\nHello.\n";
        let p = parse_skill_md(md).unwrap();
        assert_eq!(p.name, "brainstorming");
        assert_eq!(p.description, "Explore: intent, and design"); // quotes stripped, colons kept
        assert_eq!(p.body, "# Body\n\nHello.\n");
    }

    #[test]
    fn body_preserved_verbatim_including_internal_dashes() {
        let md = "---\nname: x\ndescription: d\n---\nline1\n---\nline2\n";
        let p = parse_skill_md(md).unwrap();
        assert_eq!(p.body, "line1\n---\nline2\n"); // only the FIRST closing fence splits
    }

    #[test]
    fn missing_frontmatter_is_err() {
        assert!(parse_skill_md("# no frontmatter\n").is_err());
    }

    #[test]
    fn missing_name_is_err() {
        let md = "---\ndescription: only desc\n---\nbody\n";
        assert!(parse_skill_md(md).is_err());
    }

    #[test]
    fn single_quoted_description_is_unquoted() {
        let md = "---\nname: n\ndescription: 'hi there'\n---\nb\n";
        assert_eq!(parse_skill_md(md).unwrap().description, "hi there");
    }

    #[test]
    fn all_exposes_every_skill_in_order() {
        let r = SkillRegistry::builtin();
        let names: Vec<&str> = r.all().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "spike-plan",
                "spike-implement",
                "feedback",
                "refreshing-provider-models"
            ]
        );
    }

    #[test]
    fn builtin_includes_refreshing_provider_models_skill() {
        let r = SkillRegistry::builtin();
        let s = r
            .get("refreshing-provider-models")
            .expect("refreshing-provider-models must be a built-in skill");
        assert!(
            s.description.starts_with("Use when"),
            "description must start with 'Use when'"
        );
        assert!(
            s.body.contains("ollama-cloud"),
            "skill body must mention ollama-cloud"
        );
        assert!(
            s.body.contains("/api/tags"),
            "skill body must mention /api/tags for Ollama"
        );
        assert!(
            s.body.contains("anthropic-version"),
            "skill body must mention anthropic-version header"
        );
        assert!(
            s.body.contains("MODEL_CAPS"),
            "skill body must reference MODEL_CAPS"
        );
        assert!(
            s.body.contains("opencode_zen_model_caps_present"),
            "skill body must reference the Zen caps invariant test"
        );
        assert!(
            s.body.contains("thinking_wire"),
            "skill body must reference thinking_wire"
        );
        assert!(s.base_dir.is_none(), "built-in skills have no base_dir");
    }
}
