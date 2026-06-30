//! The semantic-zoom projection (spec §4.1/①): fold the conversation into one
//! *structural* digest per turn for the zoomed-out altitude. Deterministic — no
//! model calls. `zoid-tui` renders these as one-line summaries.

use crate::economy::tool_path;
use crate::projection::ChatMsg;

/// A one-line structural summary of a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnDigest {
    pub headline: String,
    pub tools: usize,
    pub files: usize,
    pub has_error: bool,
}

const HEADLINE_MAX: usize = 60;

fn trim_headline(s: &str) -> String {
    let one_line = s.lines().next().unwrap_or("").trim();
    if one_line.chars().count() > HEADLINE_MAX {
        let head: String = one_line.chars().take(HEADLINE_MAX.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        one_line.to_string()
    }
}

/// One `TurnDigest` per turn. Turns start at each `User` message; a log that
/// opens with assistant/tool content starts an implicit turn.
pub fn digests(msgs: &[ChatMsg]) -> Vec<TurnDigest> {
    let mut out: Vec<TurnDigest> = Vec::new();
    let mut cur: Option<TurnDigest> = None;

    for m in msgs {
        match m {
            ChatMsg::User(text) => {
                if let Some(d) = cur.take() {
                    out.push(d);
                }
                cur = Some(TurnDigest {
                    headline: trim_headline(text),
                    tools: 0,
                    files: 0,
                    has_error: false,
                });
            }
            ChatMsg::Assistant { text, tool_calls } => {
                let d = cur.get_or_insert_with(|| TurnDigest {
                    headline: trim_headline(text),
                    tools: 0,
                    files: 0,
                    has_error: false,
                });
                if d.headline.is_empty() {
                    d.headline = trim_headline(text);
                }
                d.tools += tool_calls.len();
                d.files += tool_calls.iter().filter(|c| tool_path(&c.args).is_some()).count();
            }
            ChatMsg::ToolResult { is_error, .. } => {
                let d = cur.get_or_insert_with(|| TurnDigest {
                    headline: String::new(),
                    tools: 0,
                    files: 0,
                    has_error: false,
                });
                d.has_error |= *is_error;
            }
        }
    }
    if let Some(d) = cur.take() {
        out.push(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{ChatMsg, ToolCallRef};
    use proptest::prelude::*;

    fn call(name: &str, args: &str) -> ToolCallRef {
        ToolCallRef { id: String::new(), name: name.into(), args: args.into() }
    }

    #[test]
    fn one_digest_per_turn_with_counts() {
        let msgs = vec![
            ChatMsg::User("fix the parser bug".into()),
            ChatMsg::Assistant {
                text: "looking".into(),
                tool_calls: vec![
                    call("read_file", r#"{"path":"src/ast.rs"}"#), // file
                    call("shell", r#"{"command":"cargo test"}"#),  // not a file
                ],
            },
            ChatMsg::ToolResult { id: String::new(), name: "read_file".into(), output: "fn parse() {}".into(), is_error: false },
            ChatMsg::ToolResult { id: String::new(), name: "shell".into(), output: "boom".into(), is_error: true },
            ChatMsg::User("thanks".into()),
        ];
        let d = digests(&msgs);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].headline, "fix the parser bug");
        assert_eq!(d[0].tools, 2);
        assert_eq!(d[0].files, 1);
        assert!(d[0].has_error);
        assert_eq!(d[1].headline, "thanks");
        assert_eq!(d[1].tools, 0);
        assert!(!d[1].has_error);
    }

    #[test]
    fn assistant_led_log_starts_a_turn() {
        let msgs = vec![ChatMsg::Assistant { text: "hello there".into(), tool_calls: vec![] }];
        let d = digests(&msgs);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].headline, "hello there");
    }

    #[test]
    fn long_headline_is_trimmed() {
        let long = "x".repeat(200);
        let d = digests(&[ChatMsg::User(long)]);
        assert!(d[0].headline.chars().count() <= 60);
    }

    #[test]
    fn empty_log_has_no_digests() {
        assert_eq!(digests(&[]), Vec::new());
    }

    proptest! {
        #[test]
        fn never_panics_and_one_digest_per_user(n in 0usize..30) {
            let msgs: Vec<ChatMsg> = (0..n).map(|i| ChatMsg::User(format!("turn {i}"))).collect();
            prop_assert_eq!(digests(&msgs).len(), n);
        }
    }
}
