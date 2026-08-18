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
/// agent through the full model-sync workflow: run the tool, research new
/// models, update the on-disk models.toml with researched caps.
const REFRESHING_PROVIDER_MODELS_BODY: &str = concat!(
    "# Refreshing Provider Models\n\n",
    "The provider/model registry is a TOML file, not Rust code. This skill\n",
    "syncs it against live provider endpoints.\n\n",
    "## Step 1 — Run the tool\n\n",
    "```bash\n",
    "zoid refresh-models\n",
    "```\n",
    "(If testing from source: `cargo run -p zoid -- refresh-models`)\n\n",
    "The tool fetches live model lists from each provider that has a key. It\n",
    "auto-writes `wire` rows to `models.user.toml` for Ollama and Gemini (the\n",
    "only two providers with wire-derived caps endpoints). For all other\n",
    "providers (Anthropic, OpenAI-compat, opencode-go, opencode-zen, zai), it\n",
    "reports new and retired models but does NOT write them — those need\n",
    "manual caps research and `static` rows in `models.toml`.\n\n",
    "Report the output to the user: what was added, updated, removed, and what\n",
    "needs manual attention.\n\n",
    "## Step 2 — Research new models\n\n",
    "For each model in the report's `reported` list that says \"new model\"\n",
    "(needs manual caps), research its metadata:\n",
    "- `context_window` — max input tokens\n",
    "- `max_output` — max output tokens (0 = provider default)\n",
    "- `tools` — does it support tool/function calling? (default true)\n",
    "- `prompt_cache` — does it support prompt caching? (default false)\n",
    "- `thinking` — thinking support: `none`, `toggle`, `toggle-with-effort`,\n",
    "  `budget`, or `adaptive`\n",
    "- `thinking_wire` — thinking wire protocol: `none`, `anthropic`,\n",
    "  `deepseek`, `openai`, or `ollama`\n\n",
    "Use web_search to find the model's spec page (provider docs, API reference).\n",
    "If you can't find exact numbers, use conservative defaults from a sibling\n",
    "model in the same family (e.g. glm-5.3 → same caps as glm-5.2).\n\n",
    "## Step 3 — Update models.toml\n\n",
    "Add `static` rows to the shipped `models.toml` for each new model. The\n",
    "file lives at `~/.config/zoid/models.toml` (the on-disk copy seeded from\n",
    "the embedded version at boot). Edit it directly.\n\n",
    "Infer the `wire_shape` from the model family:\n\n",
    "| Model family | wire_shape |\n",
    "|---|---|\n",
    "| Claude (claude-*) | `anthropic-messages` |\n",
    "| GPT (gpt-5.*) | `openai-responses` |\n",
    "| GLM (glm-*) | `openai-chat` |\n",
    "| Grok (grok-*) | `openai-chat` |\n",
    "| Kimi (kimi-*) | `openai-chat` |\n",
    "| Deepseek (deepseek-*) | `openai-chat` |\n",
    "| Mimo (mimo-*) | `openai-chat` |\n",
    "| Minimax (minimax-*) on opencode-go | `anthropic-messages` |\n",
    "| Minimax (minimax-*) on opencode-zen | `openai-chat` |\n",
    "| Qwen (qwen3.*) on opencode-go | `anthropic-messages` |\n",
    "| Qwen (qwen3.*) on opencode-zen | `anthropic-messages` |\n",
    "| Gemini (gemini-*) | `google-gemini` |\n",
    "| Ollama (ollama-*) | `ollama` |\n",
    "| Other/unknown | `openai-chat` (conservative default) |\n\n",
    "TOML format for each row:\n\n",
    "```toml\n",
    "  [[provider.model]]\n",
    "  id = \"model-id\"\n",
    "  wire_shape = \"openai-chat\"\n",
    "  source = \"static\"\n",
    "  context_window = 1000000\n",
    "  max_output = 0\n",
    "  tools = true\n",
    "  prompt_cache = true\n",
    "  thinking = \"toggle-with-effort\"\n",
    "  thinking_wire = \"deepseek\"\n",
    "```\n\n",
    "Do NOT add `default = true` unless the user explicitly asks to change the\n",
    "provider's default model.\n\n",
    "After editing, summarize the changes for the user: which models were added\n",
    "and removed, with their researched caps. The user can confirm or ask for\n",
    "adjustments.\n\n",
    "Note: `~/.config/zoid/models.toml` is overwritten on the next zoid upgrade\n",
    "(the new binary ships a newer embedded copy). Changes you want to survive\n",
    "upgrades should go in `~/.config/zoid/models.user.toml` as `user` rows\n",
    "instead — that file is never overwritten.\n\n",
    "## What NOT to do\n\n",
    "- Do NOT add `wire` rows to `models.toml` — the shipped file is\n",
    "  `static` rows only. `wire` rows go in `models.user.toml` (written\n",
    "  automatically by the tool for Ollama + Gemini).\n",
    "- Do NOT run `cargo test` or commit — that's the user's workflow, not\n",
    "  the skill's.\n",
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
                description: "Sync zoid's provider/model registry against live \
                    endpoints: run `zoid refresh-models`, research new models, \
                    and update crates/zoid-model/models.toml with researched \
                    caps. Use when adding new models, removing retired ones, \
                    or updating provider metadata.".into(),
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
            s.description.contains("zoid refresh-models"),
            "description must mention the `zoid refresh-models` tool"
        );
        assert!(
            s.body.contains("zoid refresh-models"),
            "skill body must point at the `zoid refresh-models` tool"
        );
        assert!(
            s.body.contains("models.user.toml"),
            "skill body must name the models.user.toml output file"
        );
        assert!(
            s.body.contains("models.toml"),
            "skill body must name the shipped models.toml file"
        );
        assert!(
            s.body.contains("wire_shape"),
            "skill body must include wire_shape inference guidance"
        );
        assert!(
            s.body.contains("context_window"),
            "skill body must guide caps research"
        );
        // The repurposed body must NOT still describe the old hand-edit workflow.
        assert!(
            !s.body.contains("MODEL_CAPS"),
            "repurposed skill must not reference the old MODEL_CAPS table"
        );
        assert!(
            !s.body.contains("/api/tags"),
            "repurposed skill must not describe the old curl /api/tags workflow"
        );
        assert!(s.base_dir.is_none(), "built-in skills have no base_dir");
    }
}
