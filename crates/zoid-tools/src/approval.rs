//! Tool-call approval: a blacklist gate that prompts (or denies) on
//! dangerous shell commands. See `docs/superpowers/specs/2026-07-08-tool-approvals-design.md`.

/// A structured dangerous-command pattern. Patterns are matched against each
/// segment's token stream (or raw text for `Substring`).
#[derive(Debug, Clone)]
enum Pattern {
    /// Leading program must be exactly `prog` (e.g. "sudo", "systemctl").
    LeadingProgram { prog: String },
    /// Leading program `prog` with any of `trigger_flags` present in the
    /// token stream (e.g. curl with -X POST, -d, --data).
    ProgramWithAnyFlag { prog: String, trigger_flags: Vec<String> },
    /// Leading program `prog` with at least one flag from each of `flag_groups`
    /// present. Used when two independent flag dimensions must both be
    /// satisfied (e.g. rm needs recursive AND force).
    ProgramWithAllGroups { prog: String, flag_groups: Vec<Vec<String>> },
    /// Free-form substring match against the segment's raw text
    /// (e.g. "terraform apply", "kubectl delete").
    Substring { pattern: String },
}

impl Pattern {
    /// A human-readable label for the matched pattern (used in denial messages).
    fn label(&self) -> String {
        match self {
            Pattern::LeadingProgram { prog } => prog.clone(),
            Pattern::ProgramWithAnyFlag { prog, .. } => prog.clone(),
            Pattern::ProgramWithAllGroups { prog, .. } => prog.clone(),
            Pattern::Substring { pattern } => pattern.clone(),
        }
    }
}

/// The curated builtin dangerous patterns (all 6 categories from the design).
/// User `shell_danger` additions are appended; user `shell_allow` exemptions
/// remove matching builtin patterns.
fn builtin_defaults() -> Vec<Pattern> {
    vec![
        // Destructive rm: recursive AND force
        Pattern::ProgramWithAllGroups {
            prog: "rm".into(),
            flag_groups: vec![
                vec!["-r".into(), "--recursive".into()],
                vec!["-f".into(), "--force".into()],
            ],
        },
        // Force-push / history rewrite — match only when `git push` has a force
        // flag. Using ProgramWithAllGroups avoids false positives on `git commit -f`
        // (fixup shorthand) and `git fetch -f` (neither has `push` as a token).
        Pattern::ProgramWithAllGroups {
            prog: "git".into(),
            flag_groups: vec![
                vec!["push".into()],
                vec!["--force".into(), "-f".into(), "--force-with-lease".into()],
            ],
        },
        // Network/prod writes (curl with non-GET method or data)
        Pattern::ProgramWithAnyFlag {
            prog: "curl".into(),
            trigger_flags: vec!["-X".into(), "--data".into(), "-d".into()],
        },
        Pattern::ProgramWithAnyFlag {
            prog: "wget".into(),
            trigger_flags: vec!["--post-data".into(), "--post-file".into()],
        },
        // Privilege escalation
        Pattern::LeadingProgram { prog: "sudo".into() },
        Pattern::LeadingProgram { prog: "su".into() },
        Pattern::LeadingProgram { prog: "doas".into() },
        // System mutation
        Pattern::LeadingProgram { prog: "systemctl".into() },
        Pattern::LeadingProgram { prog: "apt".into() },
        Pattern::LeadingProgram { prog: "brew".into() },
        Pattern::ProgramWithAllGroups {
            prog: "pip".into(),
            flag_groups: vec![vec!["install".into()], vec!["--user".into()]],
        },
        Pattern::Substring { pattern: "chmod -R".into() },
        Pattern::Substring { pattern: "/etc/".into() },
        // Deploy / irrecoverable
        Pattern::Substring { pattern: "terraform apply".into() },
        Pattern::Substring { pattern: "kubectl delete".into() },
        Pattern::Substring { pattern: "fly deploy".into() },
        Pattern::Substring { pattern: "scp".into() },
        Pattern::Substring { pattern: "rsync".into() },
    ]
}

/// Split a command string on shell chain operators (`&&`, `||`, `;`, `|`) into
/// independent command segments. `||` is handled before `|` (logical-OR vs
/// pipe). Each segment is checked independently by the matcher.
///
/// The scan is quote-aware: operators inside single or double quotes — and
/// backslash-escaped operators outside quotes — are literal text, not
/// separators. This prevents a quoted regex alternation like
/// `grep -E "error|warn"` from being shredded into fragments with dangling
/// quotes, which the shlex fail-safe would otherwise flag as dangerous.
fn split_segments(cmd: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    while i < chars.len() {
        let c = chars[i];
        // Single quotes: everything is literal until the closing quote.
        if in_single {
            current.push(c);
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        // Double quotes: backslash escapes the next char; `"` closes.
        if in_double {
            if c == '\\' && i + 1 < chars.len() {
                current.push(c);
                current.push(chars[i + 1]);
                i += 2;
                continue;
            }
            current.push(c);
            if c == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        // Outside quotes: recognize quote openers, escapes, then operators.
        if c == '\'' {
            in_single = true;
            current.push(c);
            i += 1;
        } else if c == '"' {
            in_double = true;
            current.push(c);
            i += 1;
        } else if c == '\\' && i + 1 < chars.len() {
            // Escaped operator (e.g. `\|`) is literal — keep both chars.
            current.push(c);
            current.push(chars[i + 1]);
            i += 2;
        } else if i + 1 < chars.len()
            && ((c == '&' && chars[i + 1] == '&') || (c == '|' && chars[i + 1] == '|'))
        {
            // Two-char chain operators: `&&`, `||`.
            segments.push(current.clone());
            current.clear();
            i += 2;
        } else if c == ';' || c == '|' {
            // One-char separators: `;`, single pipe `|`.
            segments.push(current.clone());
            current.clear();
            i += 1;
        } else {
            current.push(c);
            i += 1;
        }
    }
    segments.push(current);
    segments
}

/// Check whether a command string matches any dangerous pattern. Splits on
/// chain operators, tokenizes each segment with shlex, then matches against
/// the pattern list. Returns the label of the first matched pattern, or `None`.
/// Fail-safe: an unparseable segment is treated as dangerous (prompt).
fn match_dangerous(cmd: &str, patterns: &[Pattern]) -> Option<String> {
    for segment in split_segments(cmd) {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(label) = match_segment(trimmed, patterns) {
            return Some(label);
        }
    }
    None
}

/// Match a single command segment against the pattern list. Returns the label
/// of the first matching pattern, or `None`.
fn match_segment(segment: &str, patterns: &[Pattern]) -> Option<String> {
    let tokens = match shlex::split(segment) {
        Some(t) if !t.is_empty() => t,
        _ => return Some(format!("unparseable: {}", segment)),
    };
    let leading = &tokens[0];

    for pattern in patterns {
        if pattern_matches(pattern, leading, &tokens, segment) {
            return Some(pattern.label());
        }
    }
    None
}

/// Check whether a single pattern matches. `leading` is tokens[0]; `segment`
/// is the raw segment text (for Substring matching).
fn pattern_matches(pattern: &Pattern, leading: &str, tokens: &[String], segment: &str) -> bool {
    match pattern {
        Pattern::LeadingProgram { prog } => leading == prog,

        Pattern::ProgramWithAnyFlag { prog, trigger_flags } => {
            if leading != prog {
                return false;
            }
            for flag in trigger_flags {
                if flag == "-X" {
                    // -X is dangerous only if followed by a non-GET method.
                    // Handle both `curl -X POST` (space-separated) and
                    // `curl -XPOST` (joined) forms.
                    for (i, tok) in tokens.iter().enumerate() {
                        if tok == "-X" && i + 1 < tokens.len() {
                            let method = tokens[i + 1].to_uppercase();
                            if method != "GET" {
                                return true;
                            }
                        }
                        if tok.starts_with("-X") && tok.len() > 2 {
                            let method = tok[2..].to_uppercase();
                            if method != "GET" {
                                return true;
                            }
                        }
                    }
                } else if tokens.contains(flag) {
                    return true;
                }
            }
            false
        }

        Pattern::ProgramWithAllGroups { prog, flag_groups } => {
            if leading != prog {
                return false;
            }
            // Each group must have at least one flag present in the tokens.
            // Handle combined short flags (e.g. `-rf` satisfies both `-r` and `-f`)
            // by checking if any token contains the flag's char after a leading `-`.
            flag_groups.iter().all(|group| {
                group.iter().any(|flag| {
                    // Exact token match (e.g. "--force" or "-r")
                    if tokens.contains(flag) {
                        return true;
                    }
                    // Combined short flag: "-rf" contains both 'r' and 'f'.
                    // Only applies to single-char short flags (e.g. "-r", "-f").
                    if flag.len() == 2 && flag.starts_with('-') && !flag.starts_with("--") {
                        let flag_char = flag.chars().nth(1).unwrap();
                        return tokens.iter().any(|tok| {
                            // Short-flag combo: "-rf" → check if flag_char is in chars after '-'
                            tok.starts_with('-')
                                && !tok.starts_with("--")
                                && tok.len() > 1
                                && tok.chars().skip(1).any(|c| c == flag_char)
                        });
                    }
                    false
                })
            })
        }

        Pattern::Substring { pattern } => segment.contains(pattern),
    }
}

/// The blacklist gate. Allow unless a `shell` call matches a dangerous pattern.
/// `interactive: true` returns `Gate::Prompt` on a match (Chat);
/// `interactive: false` returns `Gate::Deny` (subagents — headless, can't prompt).
pub struct BlacklistGate {
    patterns: Vec<Pattern>,
    interactive: bool,
}

impl BlacklistGate {
    /// Construct the gate: builtin defaults ⊕ user `shell_danger` ⊖ user
    /// `shell_allow`. User `shell_danger` entries are added as `Substring`
    /// patterns. User `shell_allow` entries exempt builtin patterns whose
    /// canonical form contains the entry string.
    pub fn new(shell_danger: Vec<String>, shell_allow: Vec<String>, interactive: bool) -> Self {
        let mut patterns: Vec<Pattern> = builtin_defaults()
            .into_iter()
            .filter(|p| !is_exempted(p, &shell_allow))
            .collect();
        for d in shell_danger {
            patterns.push(Pattern::Substring { pattern: d });
        }
        Self {
            patterns,
            interactive,
        }
    }
}

/// Check whether a builtin pattern is exempted by any `shell_allow` entry.
fn is_exempted(pattern: &Pattern, shell_allow: &[String]) -> bool {
    let canonical = pattern_canonical(pattern);
    shell_allow.iter().any(|allow| canonical.contains(allow))
}

/// A canonical string representation of a pattern, for `shell_allow` matching.
fn pattern_canonical(pattern: &Pattern) -> String {
    match pattern {
        Pattern::LeadingProgram { prog } => prog.clone(),
        Pattern::ProgramWithAnyFlag { prog, trigger_flags } => {
            format!("{} {}", prog, trigger_flags.join(" "))
        }
        Pattern::ProgramWithAllGroups { prog, flag_groups } => format!(
            "{} {}",
            prog,
            flag_groups
                .iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Pattern::Substring { pattern } => pattern.clone(),
    }
}

impl crate::ToolGate for BlacklistGate {
    fn check(&self, call: &zoid_provider::ToolCall) -> crate::Gate {
        // Never-prompt tier: always allow
        match call.name.as_str() {
            "Read" | "Grep" | "recall" | "show" | "update_tasks" | "ask_user" => {
                return crate::Gate::Allow;
            }
            _ => {}
        }
        // Allow-by-default tier: Write, Edit
        match call.name.as_str() {
            "Write" | "Edit" => return crate::Gate::Allow,
            _ => {}
        }
        // Blacklist-gated tier: shell
        if call.name == "shell" {
            let cmd = call
                .args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(label) = match_dangerous(cmd, &self.patterns) {
                if self.interactive {
                    let question = format!("`shell` calls a dangerous action — approve?\n{}", cmd);
                    return crate::Gate::Prompt {
                        question,
                        choices: vec!["approve once".into(), "deny".into()],
                    };
                } else {
                    return crate::Gate::Deny(format!(
                        "blocked by safety blacklist: matched `{}`",
                        label
                    ));
                }
            }
        }
        crate::Gate::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolGate;

    // --- split_segments tests ---

    #[test]
    fn split_segments_basic() {
        assert_eq!(split_segments("echo hi"), vec!["echo hi".to_string()]);
        assert_eq!(
            split_segments("echo hi && rm -rf /"),
            vec!["echo hi ".to_string(), " rm -rf /".to_string()]
        );
    }

    #[test]
    fn split_segments_pipe_vs_or() {
        let segs = split_segments("false || echo ok");
        assert_eq!(segs, vec!["false ".to_string(), " echo ok".to_string()]);
        let segs = split_segments("git log | grep foo");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], "git log ".to_string());
        assert_eq!(segs[1], " grep foo".to_string());
    }

    #[test]
    fn split_segments_ignores_operators_inside_quotes() {
        // Regex alternation inside a quoted grep pattern must NOT split.
        assert_eq!(
            split_segments("grep -E \"error|warn\" log"),
            vec!["grep -E \"error|warn\" log".to_string()]
        );
        assert_eq!(
            split_segments("rg 'foo|bar' ."),
            vec!["rg 'foo|bar' .".to_string()]
        );
        // Semicolons and && inside quotes are literal, not separators.
        assert_eq!(
            split_segments("grep 'a;b' f"),
            vec!["grep 'a;b' f".to_string()]
        );
        assert_eq!(
            split_segments("echo \"x && y\""),
            vec!["echo \"x && y\"".to_string()]
        );
        // A real pipe outside quotes still splits, even alongside a quoted one.
        let segs = split_segments("grep 'a|b' f | head");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0], "grep 'a|b' f ".to_string());
        assert_eq!(segs[1], " head".to_string());
    }

    #[test]
    fn match_dangerous_quoted_pipe_grep_is_safe() {
        let patterns = builtin_defaults();
        // The bug report: grep with a quoted alternation was flagged dangerous
        // via the unparseable-segment fail-safe.
        assert!(match_dangerous("grep -E \"error|warn\" log", &patterns).is_none());
        assert!(match_dangerous("rg 'foo|bar' .", &patterns).is_none());
        assert!(match_dangerous("cat x | grep 'a|b'", &patterns).is_none());
    }

    #[test]
    fn split_segments_semicolon() {
        assert_eq!(
            split_segments("cd /tmp; ls"),
            vec!["cd /tmp".to_string(), " ls".to_string()]
        );
    }

    #[test]
    fn split_segments_multiple() {
        let segs = split_segments("a && b || c ; d | e");
        assert_eq!(segs.len(), 5);
    }

    #[test]
    fn split_segments_no_separator() {
        assert_eq!(split_segments("ls -la"), vec!["ls -la".to_string()]);
    }

    // --- match_dangerous tests ---

    #[test]
    fn match_dangerous_rm_rf() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("rm -rf /", &patterns).is_some());
        assert!(match_dangerous("rm -r -f /", &patterns).is_some());
        assert!(match_dangerous("rm --recursive --force ~", &patterns).is_some());
        assert!(match_dangerous("rm file.txt", &patterns).is_none());
        assert!(match_dangerous("rm -r dist/", &patterns).is_none());
        assert!(match_dangerous("rm -f tempfile", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_git_force_push() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("git push --force", &patterns).is_some());
        assert!(match_dangerous("git push -f origin main", &patterns).is_some());
        assert!(match_dangerous("git push --force-with-lease", &patterns).is_some());
        assert!(match_dangerous("git push origin main", &patterns).is_none());
        assert!(match_dangerous("git log --oneline", &patterns).is_none());
        assert!(match_dangerous("git commit -f 123abc", &patterns).is_none());
        assert!(match_dangerous("git fetch -f", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_curl_post() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("curl -X POST localhost", &patterns).is_some());
        assert!(match_dangerous("curl -d 'data' localhost", &patterns).is_some());
        assert!(match_dangerous("curl -XPOST localhost", &patterns).is_some());
        assert!(match_dangerous("curl -XPUT localhost", &patterns).is_some());
        assert!(match_dangerous("curl -XGET localhost", &patterns).is_none());
        assert!(match_dangerous("curl -X GET localhost", &patterns).is_none());
        assert!(match_dangerous("curl localhost", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_sudo() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("sudo apt update", &patterns).is_some());
        assert!(match_dangerous("su root", &patterns).is_some());
    }

    #[test]
    fn match_dangerous_deploy() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("terraform apply", &patterns).is_some());
        assert!(match_dangerous("kubectl delete pod foo", &patterns).is_some());
        assert!(match_dangerous("fly deploy", &patterns).is_some());
    }

    #[test]
    fn match_dangerous_chained() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("echo hi && rm -rf /", &patterns).is_some());
        assert!(match_dangerous("git log | grep foo", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_quoted_rm_is_safe() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("echo \"rm -rf /\"", &patterns).is_none());
    }

    #[test]
    fn match_dangerous_unparseable_prompts() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("echo 'unterminated", &patterns).is_some());
    }

    #[test]
    fn match_dangerous_safe_commands() {
        let patterns = builtin_defaults();
        assert!(match_dangerous("ls -la", &patterns).is_none());
        assert!(match_dangerous("cargo build", &patterns).is_none());
        assert!(match_dangerous("echo hello", &patterns).is_none());
        assert!(match_dangerous("grep 'force' file", &patterns).is_none());
    }

    // --- BlacklistGate::check tests ---

    fn shell_call(cmd: &str) -> zoid_provider::ToolCall {
        zoid_provider::ToolCall {
            id: String::new(),
            name: "shell".into(),
            args: serde_json::json!({ "command": cmd }),
        }
    }

    fn tool_call(name: &str) -> zoid_provider::ToolCall {
        zoid_provider::ToolCall {
            id: String::new(),
            name: name.into(),
            args: serde_json::json!({}),
        }
    }

    #[test]
    fn gate_never_prompt_tier_always_allows() {
        let g = BlacklistGate::new(vec![], vec![], true);
        for name in ["Read", "Grep", "recall", "show", "update_tasks", "ask_user"] {
            assert_eq!(g.check(&tool_call(name)), crate::Gate::Allow, "{} must allow", name);
        }
    }

    #[test]
    fn gate_file_writes_allow_by_default() {
        let g = BlacklistGate::new(vec![], vec![], true);
        assert_eq!(g.check(&tool_call("Write")), crate::Gate::Allow);
        assert_eq!(g.check(&tool_call("Edit")), crate::Gate::Allow);
    }

    #[test]
    fn gate_safe_shell_allows() {
        let g = BlacklistGate::new(vec![], vec![], true);
        assert_eq!(g.check(&shell_call("ls -la")), crate::Gate::Allow);
    }

    #[test]
    fn gate_dangerous_shell_prompts_when_interactive() {
        let g = BlacklistGate::new(vec![], vec![], true);
        let result = g.check(&shell_call("rm -rf /"));
        assert!(matches!(result, crate::Gate::Prompt { .. }));
    }

    #[test]
    fn gate_dangerous_shell_denies_when_not_interactive() {
        let g = BlacklistGate::new(vec![], vec![], false);
        let result = g.check(&shell_call("rm -rf /"));
        assert!(matches!(result, crate::Gate::Deny(ref r) if r.contains("blocked by safety blacklist")));
    }

    #[test]
    fn gate_shell_danger_adds_custom_pattern() {
        let g = BlacklistGate::new(vec!["make deploy".into()], vec![], true);
        let result = g.check(&shell_call("make deploy"));
        assert!(matches!(result, crate::Gate::Prompt { .. }));
    }

    #[test]
    fn gate_shell_allow_exempts_builtin() {
        let g = BlacklistGate::new(vec![], vec!["--force-with-lease".into()], true);
        let result = g.check(&shell_call("git push --force-with-lease"));
        assert_eq!(result, crate::Gate::Allow);
        let result2 = g.check(&shell_call("git push --force"));
        assert_eq!(result2, crate::Gate::Allow);
    }

    #[test]
    fn gate_missing_command_arg_allows() {
        let g = BlacklistGate::new(vec![], vec![], true);
        let call = zoid_provider::ToolCall {
            id: String::new(),
            name: "shell".into(),
            args: serde_json::json!({}),
        };
        assert_eq!(g.check(&call), crate::Gate::Allow);
    }
}