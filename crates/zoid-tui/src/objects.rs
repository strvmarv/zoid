//! Object-first selection model (spec ④). Pure extraction of selectable
//! objects — files, tree-sitter symbols (via zoid-syntax, P4a), and errors —
//! from the conversation. `zoid-tui` renders these into a picker; the verb
//! table (Task 2) maps each to scoped agent verbs.

use std::collections::HashMap;
use zoid_core::economy::tool_path;
use zoid_core::projection::ChatMsg;
use zoid_syntax::{symbols, Language};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    File,
    Error,
    Symbol,
}

/// A selectable object. `target` is the prompt subject; `context` names the
/// owning file for symbols (empty otherwise).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obj {
    pub kind: ObjectKind,
    pub label: String,
    pub target: String,
    pub context: String,
}

/// Files, symbols, then errors — deterministic and de-duplicated by path.
pub fn selectable_objects(msgs: &[ChatMsg]) -> Vec<Obj> {
    // id → path, from assistant tool calls.
    let mut id_path: HashMap<&str, String> = HashMap::new();
    for m in msgs {
        if let ChatMsg::Assistant { tool_calls, .. } = m {
            for c in tool_calls {
                if let Some(p) = tool_path(&c.args) {
                    id_path.insert(c.id.as_str(), p);
                }
            }
        }
    }

    let mut files: Vec<Obj> = Vec::new();
    let mut errors: Vec<Obj> = Vec::new();
    let mut seen_paths: Vec<String> = Vec::new();
    // Latest tool-result body per path. Symbols are extracted from this AFTER the
    // pass — emitting them inside the loop would duplicate (and mix stale + fresh)
    // symbols whenever a file is read more than once (the read→edit→re-read cycle).
    let mut latest_output: HashMap<String, String> = HashMap::new();

    for m in msgs {
        if let ChatMsg::ToolResult {
            id,
            name,
            output,
            is_error,
            ..
        } = m
        {
            if *is_error {
                errors.push(Obj {
                    kind: ObjectKind::Error,
                    label: format!("error: {name}"),
                    target: output.lines().next().unwrap_or("").to_string(),
                    context: String::new(),
                });
                continue;
            }
            if let Some(path) = id_path.get(id.as_str()) {
                if !seen_paths.contains(path) {
                    seen_paths.push(path.clone());
                    files.push(Obj {
                        kind: ObjectKind::File,
                        label: path.clone(),
                        target: path.clone(),
                        context: String::new(),
                    });
                }
                // newest content wins per path.
                latest_output.insert(path.clone(), output.clone());
            }
        }
    }

    // Symbols once per unique path, in file (first-seen) order, from latest content.
    let mut syms: Vec<Obj> = Vec::new();
    for path in &seen_paths {
        if let Some(output) = latest_output.get(path) {
            for s in symbols(output, Language::from_path(path)) {
                syms.push(Obj {
                    kind: ObjectKind::Symbol,
                    label: format!("{}  ({path})", s.name),
                    target: s.name,
                    context: path.clone(),
                });
            }
        }
    }

    files.into_iter().chain(syms).chain(errors).collect()
}

/// Agent verbs scoped to an object kind (spec ④).
pub fn verbs_for(kind: ObjectKind) -> &'static [&'static str] {
    match kind {
        ObjectKind::File => &["explain", "summarize", "find usages"],
        ObjectKind::Symbol => &["explain", "find references", "add test"],
        ObjectKind::Error => &["explain", "fix"],
    }
}

/// Compose the scoped prompt a verb would run against an object. In P4d this
/// text is placed in the input box (queued); P5 dispatches it to a subagent.
pub fn verb_prompt(verb: &str, obj: &Obj) -> String {
    match obj.kind {
        ObjectKind::File => format!("{verb} the file `{}`", obj.target),
        ObjectKind::Symbol => format!("{verb} `{}` in `{}`", obj.target, obj.context),
        ObjectKind::Error => format!("{verb} this error: {}", obj.target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zoid_core::projection::{ChatMsg, ToolCallRef};

    fn call(id: &str, args: &str) -> ToolCallRef {
        ToolCallRef {
            id: id.into(),
            name: "read_file".into(),
            args: args.into(),
        }
    }

    fn seeded() -> Vec<ChatMsg> {
        vec![
            ChatMsg::User {
                text: "read the ast".into(),
                ts: 0,
            },
            ChatMsg::Assistant {
                thinking: None,
                text: String::new(),
                tool_calls: vec![call("c1", r#"{"path":"src/ast.rs"}"#)],
                ts: 0,
            },
            ChatMsg::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "fn parse() {}\nstruct Ast {}\n".into(),
                is_error: false,
                compacted: false,
                ts: 0,
            },
            ChatMsg::Assistant {
                    thinking: None,
                text: String::new(),
                tool_calls: vec![ToolCallRef {
                    id: "c2".into(),
                    name: "shell".into(),
                    args: r#"{"command":"cargo test"}"#.into(),
                }],
                ts: 0,
            },
            ChatMsg::ToolResult {
                id: "c2".into(),
                name: "shell".into(),
                output: "FAILED\n[exit 1]".into(),
                is_error: true,
                compacted: false,
                ts: 0,
            },
        ]
    }

    #[test]
    fn extracts_file_symbol_and_error_objects() {
        let objs = selectable_objects(&seeded());
        // a File object for src/ast.rs
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::File && o.target == "src/ast.rs"));
        // Symbol objects parse, scoped to the file
        assert!(objs.iter().any(|o| o.kind == ObjectKind::Symbol
            && o.target == "parse"
            && o.context == "src/ast.rs"));
        assert!(objs
            .iter()
            .any(|o| o.kind == ObjectKind::Symbol && o.target == "Ast"));
        // an Error object for the failed shell call
        assert!(objs.iter().any(|o| o.kind == ObjectKind::Error));
    }

    #[test]
    fn empty_conversation_has_no_objects() {
        assert_eq!(selectable_objects(&[]), Vec::new());
    }

    #[test]
    fn non_file_tool_results_make_no_file_object() {
        let msgs = vec![
            ChatMsg::Assistant {
                    thinking: None,
                text: String::new(),
                tool_calls: vec![ToolCallRef {
                    id: "c1".into(),
                    name: "shell".into(),
                    args: r#"{"command":"ls"}"#.into(),
                }],
                ts: 0,
            },
            ChatMsg::ToolResult {
                id: "c1".into(),
                name: "shell".into(),
                output: "a\nb".into(),
                is_error: false,
                compacted: false,
                ts: 0,
            },
        ];
        let objs = selectable_objects(&msgs);
        assert!(objs.iter().all(|o| o.kind != ObjectKind::File));
    }

    #[test]
    fn verbs_are_scoped_to_kind() {
        assert!(verbs_for(ObjectKind::Error).contains(&"fix"));
        assert!(verbs_for(ObjectKind::Symbol).contains(&"add test"));
        assert!(verbs_for(ObjectKind::File).contains(&"explain"));
    }

    #[test]
    fn verb_prompt_scopes_to_the_object() {
        let sym = Obj {
            kind: ObjectKind::Symbol,
            label: "parse  (src/ast.rs)".into(),
            target: "parse".into(),
            context: "src/ast.rs".into(),
        };
        let p = verb_prompt("explain", &sym);
        assert!(p.contains("parse"));
        assert!(p.contains("src/ast.rs"));

        let file = Obj {
            kind: ObjectKind::File,
            label: "src/ast.rs".into(),
            target: "src/ast.rs".into(),
            context: String::new(),
        };
        assert!(verb_prompt("summarize", &file).contains("src/ast.rs"));

        let err = Obj {
            kind: ObjectKind::Error,
            label: "error: shell".into(),
            target: "FAILED".into(),
            context: String::new(),
        };
        assert!(verb_prompt("fix", &err).to_lowercase().contains("fix"));
    }

    #[test]
    fn rereading_a_file_does_not_duplicate_symbols() {
        // read → (edit) → re-read of the same path: one File object, and symbols
        // come once from the LATEST content (not first+latest stacked).
        let msgs = vec![
            ChatMsg::Assistant {
                thinking: None,
                text: String::new(),
                tool_calls: vec![call("c1", r#"{"path":"src/ast.rs"}"#)],
                ts: 0,
            },
            ChatMsg::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "fn old() {}\n".into(),
                is_error: false,
                compacted: false,
                ts: 0,
            },
            ChatMsg::Assistant {
                thinking: None,
                text: String::new(),
                tool_calls: vec![call("c2", r#"{"path":"src/ast.rs"}"#)],
                ts: 0,
            },
            ChatMsg::ToolResult {
                id: "c2".into(),
                name: "read_file".into(),
                output: "fn renamed() {}\n".into(),
                is_error: false,
                compacted: false,
                ts: 0,
            },
        ];
        let objs = selectable_objects(&msgs);
        assert_eq!(
            objs.iter().filter(|o| o.kind == ObjectKind::File).count(),
            1,
            "one File per path"
        );
        let syms: Vec<&str> = objs
            .iter()
            .filter(|o| o.kind == ObjectKind::Symbol)
            .map(|o| o.target.as_str())
            .collect();
        assert_eq!(
            syms,
            vec!["renamed"],
            "symbols come once, from the latest read"
        );
    }
}
