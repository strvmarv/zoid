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
                    Use the Write tool. Then confirm in one sentence that you wrote it."
                    .into(),
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
            vec!["spike-plan".to_string(), "spike-implement".to_string()]
        );
        let plan = r.get("spike-plan").unwrap();
        assert!(
            plan.body.contains("spike-implement"),
            "spike-plan must chain to spike-implement"
        );
        assert!(plan.body.contains("invoke_skill"));
        let imp = r.get("spike-implement").unwrap();
        assert!(imp.body.contains("Write"));
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
        assert_eq!(menu.lines().count(), 2);
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
        assert_eq!(names, vec!["spike-plan", "spike-implement"]);
    }
}
