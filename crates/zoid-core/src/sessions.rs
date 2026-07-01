//! Sessions: raw `sessions`-table rows (`SessionRow`) and the folded
//! `SessionInfo` projection (see `session_list`). Pure; the store owns SQL.

use crate::event::Event;
use std::collections::HashMap;
use ulid::Ulid;

/// One row of the `sessions` table, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: Ulid,
    pub name: String,
    pub root_path: String,
    pub created_ts: i64,
    pub last_touched_ts: i64,
}

/// A session folded for the resume picker / rail widget: the row plus a
/// token total summed from that session's events (`input + output`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: Ulid,
    pub name: String,
    pub root_path: String,
    pub created_ts: i64,
    pub last_touched_ts: i64,
    pub token_total: u64,
}

/// Fold session rows into `SessionInfo`, most-recent-first by `last_touched_ts`
/// (ties broken by `id` desc for determinism). `token_total` sums each session's
/// events' `input + output`. When `root_filter` is `Some`, only sessions whose
/// `root_path` matches are returned. Pure.
pub fn session_list(rows: &[SessionRow], events: &[Event], root_filter: Option<&str>) -> Vec<SessionInfo> {
    let mut totals: HashMap<Ulid, u64> = HashMap::new();
    for e in events {
        if let Some(t) = e.tokens {
            *totals.entry(e.session_id).or_default() += t.input + t.output;
        }
    }
    let mut out: Vec<SessionInfo> = rows
        .iter()
        .filter(|r| root_filter.is_none_or(|f| r.root_path == f))
        .map(|r| SessionInfo {
            id: r.id,
            name: r.name.clone(),
            root_path: r.root_path.clone(),
            created_ts: r.created_ts,
            last_touched_ts: r.last_touched_ts,
            token_total: totals.get(&r.id).copied().unwrap_or(0),
        })
        .collect();
    out.sort_by(|a, b| b.last_touched_ts.cmp(&a.last_touched_ts).then(b.id.cmp(&a.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, TokenStat};

    fn row(id: u128, name: &str, root: &str, touched: i64) -> SessionRow {
        SessionRow { id: Ulid::from(id), name: name.into(), root_path: root.into(),
            created_ts: 0, last_touched_ts: touched }
    }
    fn usage(session: u128, input: u64, output: u64) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::Usage)
            .with_session(Ulid::from(session))
            .with_tokens(TokenStat { input, output, cached: 0 })
    }

    #[test]
    fn orders_recent_first_sums_tokens_and_filters_repo() {
        let rows = vec![
            row(1, "old", "/repo/a", 100),
            row(2, "new", "/repo/a", 300),
            row(3, "other", "/repo/b", 200),
        ];
        let events = vec![usage(1, 10, 5), usage(2, 100, 0), usage(2, 0, 40)];
        // No filter: most-recent-first across all repos, token totals folded.
        let all = session_list(&rows, &events, None);
        assert_eq!(all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["new", "other", "old"]);
        assert_eq!(all[0].token_total, 140); // session 2: 100 + 40
        assert_eq!(all[2].token_total, 15);  // session 1: 10 + 5
        // Filtered to /repo/a: drops "other".
        let a = session_list(&rows, &events, Some("/repo/a"));
        assert_eq!(a.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);
    }
}
