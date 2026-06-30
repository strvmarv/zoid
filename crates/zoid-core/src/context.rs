//! The `ContextWindow` projection (spec §8 ⑤a): the current context as a list
//! of token-spending items — system, messages, tool results, files — each with
//! a token cost, a heat heuristic, and pin/evict state folded from
//! `ContextMutation` events. Pure.

use crate::economy::{estimate_tokens, tool_path};
use crate::event::{Event, EventKind};
use std::collections::HashMap;

pub const HOT_REFS: u32 = 3;
pub const WARM_REFS: u32 = 2;
pub const COLD_RECENCY_TURNS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heat {
    Cold,
    Warm,
    Hot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    System,
    Message,
    ToolResult,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    pub key: String,
    pub label: String,
    pub kind: ItemKind,
    pub tokens: u64,
    pub heat: Heat,
    pub pinned: bool,
    pub evicted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextWindow {
    pub items: Vec<ContextItem>,
    pub total_tokens: u64,
}

// Internal accumulator while folding.
struct Acc {
    key: String,
    label: String,
    kind: ItemKind,
    tokens: u64,
    refs: u32,
    last_turn: usize,
}

pub fn context_window(events: &[Event]) -> ContextWindow {
    let mut order: Vec<String> = Vec::new(); // first-seen order of keys
    let mut acc: HashMap<String, Acc> = HashMap::new();
    let mut call_path: HashMap<String, String> = HashMap::new(); // tool id → path
    let mut turn: usize = 0;
    let mut msg_seq: usize = 0;

    let upsert = |order: &mut Vec<String>,
                  acc: &mut HashMap<String, Acc>,
                  key: String,
                  label: String,
                  kind: ItemKind,
                  tokens: u64,
                  turn: usize| {
        let e = acc.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Acc { key, label, kind, tokens: 0, refs: 0, last_turn: turn }
        });
        e.tokens = tokens; // latest content wins
        e.refs += 1;
        e.last_turn = turn;
    };

    for e in events {
        match &e.kind {
            EventKind::UserMessage { text } => {
                turn += 1;
                let key = format!("msg:{msg_seq}");
                msg_seq += 1;
                upsert(
                    &mut order,
                    &mut acc,
                    key,
                    truncate(text, 40),
                    ItemKind::Message,
                    estimate_tokens(text),
                    turn,
                );
            }
            EventKind::AssistantMessage { text } => {
                let key = format!("msg:{msg_seq}");
                msg_seq += 1;
                upsert(
                    &mut order,
                    &mut acc,
                    key,
                    truncate(text, 40),
                    ItemKind::Message,
                    estimate_tokens(text),
                    turn,
                );
            }
            EventKind::ToolCall { id, args, .. } => {
                if let Some(path) = tool_path(args) {
                    call_path.insert(id.clone(), path.clone());
                    // a call targeting a known file counts as a reference (drives heat)
                    let key = format!("file:{path}");
                    if let Some(a) = acc.get_mut(&key) {
                        a.refs += 1;
                        a.last_turn = turn;
                    }
                }
            }
            EventKind::ToolResult { id, name, output, .. } => {
                if let Some(path) = call_path.get(id) {
                    let key = format!("file:{path}");
                    upsert(
                        &mut order,
                        &mut acc,
                        key,
                        path.clone(),
                        ItemKind::File,
                        estimate_tokens(output),
                        turn,
                    );
                } else {
                    let key = format!("tool:{name}:{id}");
                    upsert(
                        &mut order,
                        &mut acc,
                        key,
                        name.clone(),
                        ItemKind::ToolResult,
                        estimate_tokens(output),
                        turn,
                    );
                }
            }
            _ => {}
        }
    }

    let last_turn_global = turn;
    let mut items: Vec<ContextItem> = order
        .iter()
        .map(|k| {
            let a = &acc[k];
            ContextItem {
                key: a.key.clone(),
                label: a.label.clone(),
                kind: a.kind,
                tokens: a.tokens,
                heat: heat_of(a.refs, a.last_turn, last_turn_global),
                pinned: false,
                evicted: false,
            }
        })
        .collect();

    // Fold mutations (log order; last write wins per flag).
    for e in events {
        if let EventKind::ContextMutation { item, op } = &e.kind {
            if let Some(it) = items.iter_mut().find(|i| &i.key == item) {
                use crate::event::MutationOp::*;
                match op {
                    Pin => it.pinned = true,
                    Unpin => it.pinned = false,
                    Evict => it.evicted = true,
                    Restore => it.evicted = false,
                }
            }
        }
    }

    // Sort by tokens desc, then key asc (deterministic for snapshots).
    items.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.key.cmp(&b.key)));
    let total_tokens = items.iter().map(|i| i.tokens).sum();
    ContextWindow { items, total_tokens }
}

fn heat_of(refs: u32, last_turn: usize, current_turn: usize) -> Heat {
    let recency = current_turn.saturating_sub(last_turn);
    if refs >= HOT_REFS || recency == 0 {
        Heat::Hot
    } else if refs >= WARM_REFS || recency <= COLD_RECENCY_TURNS {
        Heat::Warm
    } else {
        Heat::Cold
    }
}

fn truncate(s: &str, max: usize) -> String {
    let one_line = s.lines().next().unwrap_or("");
    if one_line.chars().count() > max {
        let head: String = one_line.chars().take(max.saturating_sub(1)).collect();
        format!("{head}\u{2026}")
    } else {
        one_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, EventKind, MutationOp};
    use ulid::Ulid;

    fn ev(kind: EventKind) -> Event {
        Event::new(Ulid::new(), None, 0, kind)
    }
    fn u(t: &str) -> Event {
        ev(EventKind::UserMessage { text: t.into() })
    }
    fn call(id: &str, name: &str, path: &str) -> Event {
        ev(EventKind::ToolCall {
            id: id.into(),
            name: name.into(),
            args: format!(r#"{{"path":"{path}"}}"#),
        })
    }
    fn result(id: &str, name: &str, out: &str) -> Event {
        ev(EventKind::ToolResult {
            id: id.into(),
            name: name.into(),
            output: out.into(),
            is_error: false,
        })
    }

    #[test]
    fn window_has_items_across_kinds() {
        let evs = vec![
            u("read the config"),
            call("c1", "read_file", "cfg.toml"),
            result("c1", "read_file", "key = 1\nkey2 = 2\n"),
            // shell call has no path key → ToolResult, not File
            ev(EventKind::ToolCall {
                id: "c2".into(),
                name: "shell".into(),
                args: r#"{"command":"echo hi"}"#.into(),
            }),
            result("c2", "shell", "lots of shell output here"),
        ];
        let w = context_window(&evs);
        let kinds: Vec<ItemKind> = w.items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&ItemKind::Message));
        assert!(kinds.contains(&ItemKind::File));
        assert!(kinds.contains(&ItemKind::ToolResult));
        // File item keyed by path, with positive token estimate.
        let f = w.items.iter().find(|i| i.kind == ItemKind::File).unwrap();
        assert_eq!(f.key, "file:cfg.toml");
        assert!(f.tokens > 0);
        // Sorted by tokens desc.
        for pair in w.items.windows(2) {
            assert!(pair[0].tokens >= pair[1].tokens);
        }
        assert_eq!(w.total_tokens, w.items.iter().map(|i| i.tokens).sum::<u64>());
    }

    #[test]
    fn pin_and_evict_fold_onto_items() {
        let evs = vec![
            u("go"),
            call("c1", "read_file", "a.rs"),
            result("c1", "read_file", "fn main() {}"),
            ev(EventKind::ContextMutation { item: "file:a.rs".into(), op: MutationOp::Pin }),
            ev(EventKind::ContextMutation { item: "file:a.rs".into(), op: MutationOp::Evict }),
        ];
        let w = context_window(&evs);
        let a = w.items.iter().find(|i| i.key == "file:a.rs").unwrap();
        assert!(a.pinned);
        assert!(a.evicted); // both flags set; precedence resolved by the assembler
    }

    #[test]
    fn repeated_reads_make_a_file_hot_single_item() {
        let mut evs = vec![u("go")];
        for i in 0..3 {
            evs.push(call(&format!("c{i}"), "read_file", "hot.rs"));
            evs.push(result(&format!("c{i}"), "read_file", "fn x() {}"));
        }
        let w = context_window(&evs);
        let hot: Vec<_> = w.items.iter().filter(|i| i.key == "file:hot.rs").collect();
        assert_eq!(hot.len(), 1, "reads of one path collapse to one item");
        assert_eq!(hot[0].heat, Heat::Hot);
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn total_equals_sum_and_sorted_desc(n in 0usize..30) {
            let mut evs = vec![u("seed")];
            for i in 0..n {
                evs.push(call(&format!("c{i}"), "read_file", &format!("f{}.rs", i % 5)));
                evs.push(result(&format!("c{i}"), "read_file", &"x".repeat(i + 1)));
            }
            let w = context_window(&evs);
            prop_assert_eq!(w.total_tokens, w.items.iter().map(|i| i.tokens).sum::<u64>());
            // sorted desc
            for pair in w.items.windows(2) {
                prop_assert!(pair[0].tokens >= pair[1].tokens);
            }
        }
    }
}
