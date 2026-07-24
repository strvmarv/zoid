//! The `ContextWindow` projection (spec §8 ⑤a): the current context as a list
//! of token-spending items — system, messages, tool results, files — each with
//! a token cost, a heat heuristic, and pin/evict state folded from
//! `ContextMutation` events. Pure.

use crate::economy::{estimate_tokens, tool_path};
use crate::event::{Event, EventKind};
use std::collections::HashMap;

/// Tokens sent to the provider that are NOT derivable from the event log:
/// the system prompt and the tool spec schemas. These are constant for a
/// turn but absent from the event stream, so `context_window` can't infer
/// them. The caller (agent loop) supplies them so the window's `total_tokens`
/// reflects the full request size, not just the conversation items.
#[derive(Debug, Clone, Default)]
pub struct ContextOverhead {
    /// Estimated tokens in the system prompt (0 if none).
    pub system_tokens: u64,
    /// Estimated tokens across all tool spec JSON schemas (0 if no tools).
    pub tools_tokens: u64,
}

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
    /// Set when a `ToolResultCompacted` event has folded over this item; its
    /// `tokens` then reflect the summary size, not the original.
    pub compacted: bool,
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

fn upsert(
    order: &mut Vec<String>,
    acc: &mut HashMap<String, Acc>,
    key: String,
    label: String,
    kind: ItemKind,
    tokens: u64,
    turn: usize,
) {
    let e = acc.entry(key.clone()).or_insert_with(|| {
        order.push(key.clone());
        Acc {
            key,
            label,
            kind,
            tokens: 0,
            refs: 0,
            last_turn: turn,
        }
    });
    e.tokens = tokens; // latest content wins
    e.refs += 1;
    e.last_turn = turn;
}

/// Flush any accumulated ModelDelta text as a single assistant Message item.
/// `pending_call_args_tokens` is the token cost of any tool-call args in this
/// assistant turn (sent to the provider as serialized tool_calls); it's folded
/// into the message's token estimate and reset.
fn flush_delta(
    delta_text: &mut Option<String>,
    order: &mut Vec<String>,
    acc: &mut HashMap<String, Acc>,
    msg_seq: &mut usize,
    turn: usize,
    pending_call_args_tokens: &mut u64,
) {
    if let Some(text) = delta_text.take() {
        let key = format!("msg:{msg_seq}");
        *msg_seq += 1;
        let tokens = estimate_tokens(&text) + *pending_call_args_tokens;
        *pending_call_args_tokens = 0;
        upsert(
            order,
            acc,
            key,
            truncate(&text, 40),
            ItemKind::Message,
            tokens,
            turn,
        );
    } else if *pending_call_args_tokens > 0 {
        // Tool calls with no preceding text delta still constitute an assistant
        // turn whose serialized tool_calls cost tokens.
        let key = format!("msg:{msg_seq}");
        *msg_seq += 1;
        let tokens = *pending_call_args_tokens;
        *pending_call_args_tokens = 0;
        upsert(
            order,
            acc,
            key,
            "(tool calls)".to_string(),
            ItemKind::Message,
            tokens,
            turn,
        );
    }
}

/// Extract the tool-call id from a non-file context-item key.
/// Non-file tool-result keys are formatted `"tool:{name}:{id}"` (see the
/// `format!("tool:{name}:{id}")` in `context_window`), so the id is the segment
/// after the final `:`. Returns `None` for keys with no `:` (never a tool key).
pub fn tool_id_of(key: &str) -> Option<&str> {
    key.rsplit_once(':').map(|(_, id)| id)
}

/// Project the MAIN branch's context window (spec §8 ⑤a). Subagent work lives
/// on its own `subagent:<id>` branch (mirrors `conversation()` in
/// `projection.rs`) and is not part of the main conversation's actual
/// context, so non-default-branch events are skipped entirely here.
///
/// `TurnsDropped` markers are NOT applied here. Layer-4 turn-dropping was
/// removed (it cascaded and wiped history — see `compaction.rs`); old
/// `TurnsDropped` events in existing DBs are now inert metadata, never
/// filtering the window. This also ensures the economy/compaction view
/// reflects the same full history the transcript and model request see.
///
/// `overhead` carries the system prompt + tool spec token costs — tokens the
/// provider counts against the context ceiling but which are not derivable
/// from the event log. They are folded into `total_tokens` (and an `ItemKind::System`
/// item is emitted so the window is honest about the full request size).
pub fn context_window<'a>(events: impl IntoIterator<Item = &'a Event>) -> ContextWindow {
    context_window_with(events, ContextOverhead::default())
}

/// Like `context_window` but with caller-supplied overhead (system prompt +
/// tool specs). The overhead is added as a single `System` item and folded
/// into `total_tokens`, so the window reflects the full request the provider
/// actually tokenizes — not just the conversation items.
pub fn context_window_with<'a>(
    events: impl IntoIterator<Item = &'a Event>,
    overhead: ContextOverhead,
) -> ContextWindow {
    let events: Vec<&Event> = events.into_iter().collect();
    let evicted = crate::eviction::evicted_ids(events.iter().copied());
    let visible: &[&Event] = &events;

    let mut order: Vec<String> = Vec::new(); // first-seen order of keys
    let mut acc: HashMap<String, Acc> = HashMap::new();
    let mut call_path: HashMap<String, String> = HashMap::new(); // tool id → path
    let mut turn: usize = 0;
    let mut msg_seq: usize = 0;
    let mut delta_text: Option<String> = None; // accumulates consecutive ModelDelta runs
    let mut pending_call_args_tokens: u64 = 0; // tool-call args cost, folded into the assistant message

    for e in visible {
        if evicted.contains(&e.id) {
            continue;
        }
        // Subagent work lives on its own branch and is not part of the main
        // context window (mirrors `conversation()`); only main-branch events count.
        if e.branch != crate::event::BranchId::default() {
            continue;
        }
        match &e.kind {
            EventKind::UserMessage { text } => {
                flush_delta(
                    &mut delta_text,
                    &mut order,
                    &mut acc,
                    &mut msg_seq,
                    turn,
                    &mut pending_call_args_tokens,
                );
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
                flush_delta(
                    &mut delta_text,
                    &mut order,
                    &mut acc,
                    &mut msg_seq,
                    turn,
                    &mut pending_call_args_tokens,
                );
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
            EventKind::ModelDelta { text } => {
                // Accumulate into the current assistant turn; flushed at boundaries.
                delta_text.get_or_insert_with(String::new).push_str(text);
            }
            EventKind::ToolCall { id, args, .. } => {
                if let Some(path) = tool_path(args) {
                    // Record which path this call targets so the paired ToolResult
                    // can upsert a File item. The ToolResult upsert is the sole
                    // place refs is incremented (one ref per read, not two).
                    call_path.insert(id.clone(), path.clone());
                }
                // The tool-call args JSON is sent to the provider as part of
                // the assistant message (serialized tool_calls). Track its
                // token cost to fold into the current assistant message item
                // when it flushes.
                pending_call_args_tokens += estimate_tokens(args);
            }
            EventKind::ToolResult {
                id, name, output, ..
            } => {
                flush_delta(
                    &mut delta_text,
                    &mut order,
                    &mut acc,
                    &mut msg_seq,
                    turn,
                    &mut pending_call_args_tokens,
                );
                if let Some(path) = call_path.get(id) {
                    let key = format!("file:{path}");
                    let path = path.clone();
                    upsert(
                        &mut order,
                        &mut acc,
                        key,
                        path,
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
    // Flush any trailing assistant delta after the last event.
    flush_delta(
        &mut delta_text,
        &mut order,
        &mut acc,
        &mut msg_seq,
        turn,
        &mut pending_call_args_tokens,
    );

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
                compacted: false,
            }
        })
        .collect();

    // Fold mutations + compactions (log order; last write wins per item).
    // Only visible events contribute — a compaction or mutation from a
    // dropped turn must not fold onto items that survived the drop.
    for e in visible {
        match &e.kind {
            EventKind::ContextMutation { item, op } => {
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
            EventKind::ToolResultCompacted { id, summary, .. } => {
                // Item keys for non-file tool results are "tool:{name}:{id}";
                // a compacted File item is instead keyed "file:{path}" — resolve
                // via the same `call_path` map the File upsert used, so a
                // compacted file's window tokens are redirected to the summary
                // too (compaction covers File items, not just ToolResult).
                let file_key = call_path.get(id).map(|p| format!("file:{p}"));
                if let Some(it) = items.iter_mut().find(|i| {
                    (i.kind == ItemKind::ToolResult && tool_id_of(&i.key) == Some(id.as_str()))
                        || file_key.as_deref() == Some(i.key.as_str())
                }) {
                    it.tokens = crate::economy::estimate_tokens(summary);
                    it.compacted = true;
                }
            }
            _ => {}
        }
    }

    // Prepend the overhead (system prompt + tool specs) as a single System
    // item. It's always present in every request and counts against the
    // context ceiling, so the window must reflect it.
    let overhead_tokens = overhead.system_tokens + overhead.tools_tokens;
    if overhead_tokens > 0 {
        items.insert(
            0,
            ContextItem {
                key: "system+tools".into(),
                label: "system + tools".into(),
                kind: ItemKind::System,
                tokens: overhead_tokens,
                heat: Heat::Hot, // always present → always hot
                pinned: false,
                evicted: false,
                compacted: false,
            },
        );
    }

    // Re-sort by tokens desc, then key asc (deterministic for snapshots).
    items.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.key.cmp(&b.key)));
    let total_tokens = items.iter().map(|i| i.tokens).sum();
    ContextWindow {
        items,
        total_tokens,
    }
}

/// Resolve each File context item to its content: `"file:{path}"` → the latest
/// non-error tool-result output for that path. Mirrors `context_window`'s File
/// keying so a `ContextItem.key` looks up here. Used by the subagent context
/// builder (P5) to fetch relevant code WITHOUT the chat transcript.
pub fn file_contents<'a>(events: impl IntoIterator<Item = &'a Event>) -> HashMap<String, String> {
    let events: Vec<&Event> = events.into_iter().collect();
    let visible: &[&Event] = &events;
    // Compacted (id → summary): a compacted file read must inline its summary,
    // never the raw body — mirrors `projection.rs`'s `conversation()` fold, and
    // is required so #6b (clearing a compacted body) can't hand a subagent an
    // empty file.
    let compacted: HashMap<&str, &str> = visible
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResultCompacted { id, summary, .. } => {
                Some((id.as_str(), summary.as_str()))
            }
            _ => None,
        })
        .collect();
    let mut call_path: HashMap<String, String> = HashMap::new(); // tool id → path
    let mut out: HashMap<String, String> = HashMap::new();
    for e in visible {
        match &e.kind {
            EventKind::ToolCall { id, args, .. } => {
                if let Some(p) = tool_path(args) {
                    call_path.insert(id.clone(), p);
                }
            }
            EventKind::ToolResult {
                id,
                output,
                is_error: false,
                ..
            } => {
                if let Some(p) = call_path.get(id) {
                    let body = match compacted.get(id.as_str()) {
                        Some(sum) => (*sum).to_string(),
                        None => output.clone(),
                    };
                    out.insert(format!("file:{p}"), body); // latest wins
                }
            }
            _ => {}
        }
    }
    out
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
    fn file_contents_resolves_latest_output_by_path_key() {
        let evs = vec![
            u("go"),
            call("c1", "read_file", "src/a.rs"),
            result("c1", "read_file", "fn one() {}"),
            call("c2", "read_file", "src/a.rs"), // re-read → latest wins
            result("c2", "read_file", "fn two() {}"),
            call("c3", "read_file", "src/b.rs"),
            result("c3", "read_file", "// b"),
            // a non-file tool result must NOT be keyed as a file
            ev(EventKind::ToolCall {
                id: "c4".into(),
                name: "shell".into(),
                args: r#"{"command":"ls"}"#.into(),
            }),
            ev(EventKind::ToolResult {
                id: "c4".into(),
                name: "shell".into(),
                output: "out".into(),
                is_error: false,
            }),
        ];
        let map = file_contents(&evs);
        assert_eq!(
            map.get("file:src/a.rs").map(String::as_str),
            Some("fn two() {}")
        );
        assert_eq!(map.get("file:src/b.rs").map(String::as_str), Some("// b"));
        assert!(!map.keys().any(|k| k.starts_with("tool:")));
    }

    #[test]
    fn file_contents_skips_errored_results() {
        let evs = vec![
            u("go"),
            call("c1", "read_file", "x.rs"),
            ev(EventKind::ToolResult {
                id: "c1".into(),
                name: "read_file".into(),
                output: "boom".into(),
                is_error: true,
            }),
        ];
        let map = file_contents(&evs);
        assert!(!map.contains_key("file:x.rs"));
    }

    #[test]
    fn file_contents_substitutes_summary_for_compacted_file() {
        let evs = vec![
            call("call-1", "read_file", "/src/x.rs"),
            result("call-1", "read_file", "FULL FILE BODY"),
            ev(EventKind::ToolResultCompacted {
                id: "call-1".into(),
                summary: "file summary".into(),
                original_tokens: 500,
            }),
        ];
        let map = file_contents(&evs);
        assert_eq!(
            map.get("file:/src/x.rs").map(String::as_str),
            Some("file summary"),
            "compacted file must inline its summary, never the raw/cleared body"
        );
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
        assert_eq!(
            w.total_tokens,
            w.items.iter().map(|i| i.tokens).sum::<u64>()
        );
    }

    #[test]
    fn pin_and_evict_fold_onto_items() {
        let evs = vec![
            u("go"),
            call("c1", "read_file", "a.rs"),
            result("c1", "read_file", "fn main() {}"),
            ev(EventKind::ContextMutation {
                item: "file:a.rs".into(),
                op: MutationOp::Pin,
            }),
            ev(EventKind::ContextMutation {
                item: "file:a.rs".into(),
                op: MutationOp::Evict,
            }),
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

    /// FIX 1: Each ToolResult upsert is the only refs increment.
    /// Two reads → refs=2 → Warm (not Hot). Under the old double-count it
    /// would be refs=3 → Hot, so this test discriminates the fix.
    #[test]
    fn two_reads_then_idle_is_warm_not_hot() {
        // Two reads of the same file in turn 1.
        let mut evs = vec![u("go")];
        evs.push(call("c0", "read_file", "warm.rs"));
        evs.push(result("c0", "read_file", "fn x() {}"));
        evs.push(call("c1", "read_file", "warm.rs"));
        evs.push(result("c1", "read_file", "fn x() {}"));
        // 4 idle user-message turns push recency well past COLD_RECENCY_TURNS=3.
        for i in 0..4 {
            evs.push(u(&format!("idle {i}")));
        }
        let w = context_window(&evs);
        let item = w.items.iter().find(|i| i.key == "file:warm.rs").unwrap();
        // refs==2 → Warm; recency==4 > COLD_RECENCY_TURNS → not Hot by recency.
        assert_eq!(
            item.heat,
            Heat::Warm,
            "two reads should yield Warm, not Hot (refs=2)"
        );
    }

    /// FIX 2: A run of ModelDelta events collapses into exactly one Message item.
    #[test]
    fn model_delta_run_becomes_one_message_item() {
        let evs = vec![
            u("q"),
            ev(EventKind::ModelDelta { text: "hel".into() }),
            ev(EventKind::ModelDelta { text: "lo".into() }),
            // Non-file ToolResult acts as a boundary that flushes the delta.
            ev(EventKind::ToolResult {
                id: "x".into(),
                name: "shell".into(),
                output: "ok".into(),
                is_error: false,
            }),
        ];
        let w = context_window(&evs);
        let messages: Vec<_> = w
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::Message)
            .collect();
        // Exactly 2 Message items: user "q" and assistant "hello".
        assert_eq!(
            messages.len(),
            2,
            "expected user msg + one collapsed assistant msg"
        );
        let assistant = messages
            .iter()
            .find(|i| i.label.contains("hello"))
            .unwrap_or_else(|| {
                panic!("no Message item with label containing 'hello'; items: {messages:?}")
            });
        assert_eq!(assistant.kind, ItemKind::Message);
    }

    /// FIX (CL2): a subagent-branch tool result must not be counted in the
    /// main session's context window (mirrors `conversation()`'s branch skip).
    #[test]
    fn context_window_skips_non_main_branch_events() {
        use crate::event::BranchId;
        let mut sub = call("s1", "read_file", "sub/secret.rs");
        sub.branch = BranchId("subagent:x".into());
        let mut sub_res = result("s1", "read_file", "fn hidden() {}");
        sub_res.branch = BranchId("subagent:x".into());
        let evs = vec![
            u("main task"),
            call("m1", "read_file", "main.rs"),
            result("m1", "read_file", "fn main() {}"),
            sub,
            sub_res,
        ];
        let w = context_window(&evs);
        assert!(w.items.iter().any(|i| i.key == "file:main.rs"));
        assert!(
            !w.items.iter().any(|i| i.key == "file:sub/secret.rs"),
            "subagent-branch file excluded"
        );
    }

    #[test]
    fn context_window_folds_tool_result_compaction() {
        use crate::economy::estimate_tokens;
        let big: String = (0..200).map(|i| format!("row {i}\n")).collect();
        let summary = "row 0\n… (compacted: 199 more lines, ~700 tokens elided)".to_string();
        let evs = vec![
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::UserMessage { text: "go".into() },
            ),
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::ToolCall {
                    id: "c1".into(),
                    name: "search".into(),
                    args: "{}".into(),
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::ToolResult {
                    id: "c1".into(),
                    name: "search".into(),
                    output: big,
                    is_error: false,
                },
            ),
            Event::new(
                Ulid::new(),
                None,
                0,
                EventKind::ToolResultCompacted {
                    id: "c1".into(),
                    summary: summary.clone(),
                    original_tokens: 999,
                },
            ),
        ];
        let w = context_window(&evs);
        let it = w
            .items
            .iter()
            .find(|i| i.key == "tool:search:c1")
            .expect("tool item present");
        assert!(it.compacted);
        assert_eq!(it.tokens, estimate_tokens(&summary));
    }

    #[test]
    fn context_window_overrides_compacted_file_tokens() {
        use crate::economy::estimate_tokens;
        let summary = "y summary".to_string();
        let evs = vec![
            call("call-2", "read_file", "/src/y.rs"),
            result("call-2", "read_file", &"x".repeat(3000)),
            ev(EventKind::ToolResultCompacted {
                id: "call-2".into(),
                summary: summary.clone(),
                original_tokens: 1000,
            }),
        ];
        let w = context_window(&evs);
        let file_item = w
            .items
            .iter()
            .find(|i| i.key == "file:/src/y.rs")
            .expect("file item present");
        assert_eq!(
            file_item.tokens,
            estimate_tokens(&summary),
            "compacted file item weighs its summary, not the raw body or 0"
        );
        assert!(file_item.compacted);
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
            // sorted desc by tokens, tiebroken asc by key
            for pair in w.items.windows(2) {
                prop_assert!(
                    pair[0].tokens > pair[1].tokens
                        || (pair[0].tokens == pair[1].tokens && pair[0].key <= pair[1].key)
                );
            }
        }
    }

    #[test]
    fn context_window_excludes_evicted_tokens() {
        use crate::event::{Event, EventKind, EvictionMarker};
        use ulid::Ulid;
        let big = "x".repeat(3000); // ~1000 tokens
        let base = vec![Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: big.clone() },
        )];
        let with_evict = vec![
            base[0].clone(),
            Event::new(
                Ulid::from(9u128),
                None,
                9,
                EventKind::TurnsEvicted {
                    ids: vec![Ulid::from(1u128)],
                    reclaimed_tokens: 1000,
                    marker: EvictionMarker { spans: vec![] },
                    rescue: None,
                },
            ),
        ];
        let full = context_window_with(&base, ContextOverhead::default()).total_tokens;
        let after = context_window_with(&with_evict, ContextOverhead::default()).total_tokens;
        assert!(
            after < full,
            "evicted event's tokens must be excluded from the window"
        );
    }

    #[test]
    fn context_window_ignores_directive_reasserted() {
        let base = vec![u("hello world this is content")];
        let mut with_marker = base.clone();
        with_marker.push(ev(EventKind::DirectiveReasserted { at_cumulative: 999 }));
        assert_eq!(
            context_window(&base).total_tokens,
            context_window(&with_marker).total_tokens,
            "re-floor marker must not change the context window total"
        );
    }
}
