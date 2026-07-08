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
    /// Multi-instance safety (spec §2.2): an interface has this session open.
    pub active: bool,
    /// The OS PID of the interface holding this session, or None when inactive.
    pub active_pid: Option<i64>,
    /// Epoch-ms the holder last refreshed its liveness, or None when inactive.
    pub active_heartbeat: Option<i64>,
}

/// A session folded for the resume picker / rail widget: the row plus a
/// token total summed from that session's events. The total excludes
/// cache-read tokens (net new tokens only: `(input - cached) + output`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: Ulid,
    pub name: String,
    pub root_path: String,
    pub created_ts: i64,
    pub last_touched_ts: i64,
    pub token_total: u64,
    /// Multi-instance safety (spec §2.2): an interface has this session open.
    pub active: bool,
    /// The OS PID of the interface holding this session, or None when inactive.
    pub active_pid: Option<i64>,
    /// Epoch-ms the holder last refreshed its liveness, or None when inactive.
    pub active_heartbeat: Option<i64>,
}

/// Fold session rows into `SessionInfo`, most-recent-first by `last_touched_ts`
/// (ties broken by `id` desc for determinism). `token_total` sums each session's
/// events' `(input - cached) + output` — cache-read tokens are excluded so the
/// total reflects net new tokens, not re-sent cached content. When
/// `root_filter` is `Some`, only sessions whose `root_path` matches are
/// returned. Pure.
pub fn session_list(
    rows: &[SessionRow],
    events: &[Event],
    root_filter: Option<&str>,
) -> Vec<SessionInfo> {
    let mut totals: HashMap<Ulid, u64> = HashMap::new();
    for e in events {
        if let Some(t) = e.tokens {
            let net = t.input.saturating_sub(t.cached) + t.output;
            *totals.entry(e.session_id).or_default() += net;
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
            active: r.active,
            active_pid: r.active_pid,
            active_heartbeat: r.active_heartbeat,
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_touched_ts
            .cmp(&a.last_touched_ts)
            .then(b.id.cmp(&a.id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, TokenStat};

    fn row(id: u128, name: &str, root: &str, touched: i64) -> SessionRow {
        SessionRow {
            id: Ulid::from(id),
            name: name.into(),
            root_path: root.into(),
            created_ts: 0,
            last_touched_ts: touched,
            active: false,
            active_pid: None,
            active_heartbeat: None,
        }
    }
    fn usage(session: u128, input: u64, output: u64) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::Usage)
            .with_session(Ulid::from(session))
            .with_tokens(TokenStat {
                thinking: 0,
                input,
                output,
                cached: 0,
            })
    }
    fn usage_cached(session: u128, input: u64, cached: u64, output: u64) -> Event {
        Event::new(Ulid::new(), None, 0, EventKind::Usage)
            .with_session(Ulid::from(session))
            .with_tokens(TokenStat {
                thinking: 0,
                input,
                output,
                cached,
            })
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
        assert_eq!(
            all.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["new", "other", "old"]
        );
        assert_eq!(all[0].token_total, 140); // session 2: 100 + 40
        assert_eq!(all[2].token_total, 15); // session 1: 10 + 5
                                            // Filtered to /repo/a: drops "other".
        let a = session_list(&rows, &events, Some("/repo/a"));
        assert_eq!(
            a.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["new", "old"]
        );
    }

    #[test]
    fn cached_tokens_excluded_from_total() {
        // input=200, cached=150, output=50 → net = (200-150)+50 = 100
        let rows = vec![row(1, "cached", "/repo", 100)];
        let events = vec![usage_cached(1, 200, 150, 50)];
        let all = session_list(&rows, &events, None);
        assert_eq!(all[0].token_total, 100); // net, not 250
    }

    #[test]
    fn session_list_carries_liveness_columns() {
        let mut r = row(1, "a", "/repo", 100);
        r.active = true;
        r.active_pid = Some(42);
        r.active_heartbeat = Some(1000);
        let rows = vec![r];
        let list = session_list(&rows, &[], None);
        assert!(list[0].active);
        assert_eq!(list[0].active_pid, Some(42));
        assert_eq!(list[0].active_heartbeat, Some(1000));
    }
}
