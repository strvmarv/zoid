use crate::event::{BranchId, Event};
use crate::sessions::SessionRow;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use ulid::Ulid;

/// Single-writer, append-only event log backed by SQLite. The store owns the
/// connection; readers obtain owned `Vec<Event>` snapshots via `load_all`.
pub struct EventStore {
    conn: Connection,
}

/// The searchable text of an event, or None for content-less events (Usage,
/// eviction markers, tasks). Indexed into `events_fts` at append.
fn fts_content(kind: &crate::event::EventKind) -> Option<String> {
    use crate::event::EventKind::*;
    match kind {
        UserMessage { text } | AssistantMessage { text } | ModelDelta { text } => {
            Some(text.clone())
        }
        ToolResult { output, name, .. } => Some(format!("{name}\n{output}")),
        ToolCall { name, args, .. } => Some(format!("{name} {args}")),
        DelegationResult { summary, .. } => Some(summary.clone()),
        _ => None,
    }
}

impl EventStore {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        // WAL allows concurrent readers + one writer without blocking (spec §1).
        // `busy_timeout` makes a contended writer retry for 5s before returning
        // SQLITE_BUSY — turns "two zoids → random turn failures" into brief stalls.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id         TEXT PRIMARY KEY,
                parent     TEXT,
                branch     TEXT NOT NULL,
                session_id TEXT NOT NULL,
                ts         INTEGER NOT NULL,
                kind       TEXT NOT NULL,
                tokens     TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id);
            CREATE TABLE IF NOT EXISTS sessions (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                root_path       TEXT NOT NULL,
                created_ts      INTEGER NOT NULL,
                last_touched_ts INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS secrets (
                name        TEXT PRIMARY KEY,
                ciphertext  BLOB NOT NULL,
                nonce       BLOB NOT NULL,
                created_ts  INTEGER NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
                content,
                event_id UNINDEXED,
                session_id UNINDEXED
            );
            CREATE TABLE IF NOT EXISTS event_embeddings (
                event_id  TEXT NOT NULL,
                model_id  TEXT NOT NULL,
                dim       INTEGER NOT NULL,
                vector    BLOB NOT NULL,
                PRIMARY KEY (event_id, model_id)
            );",
        )?;
        // First-ever schema migration (spec §11). `CREATE TABLE IF NOT EXISTS`
        // above is a no-op for an existing DB, so a NEW column must be added with
        // ALTER TABLE — probed so re-open is idempotent (SQLite has no
        // ADD COLUMN IF NOT EXISTS).
        let has_active_mode: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'active_mode'",
            [],
            |r| r.get(0),
        )?;
        if has_active_mode == 0 {
            conn.execute("ALTER TABLE sessions ADD COLUMN active_mode TEXT", [])?;
        }
        // Liveness columns (multi-instance safety spec §2.2). Idempotent —
        // probe-then-ALTER, mirroring `active_mode` above (SQLite has no ADD
        // COLUMN IF NOT EXISTS).
        let has_active: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'active'",
            [],
            |r| r.get(0),
        )?;
        if has_active == 0 {
            conn.execute("ALTER TABLE sessions ADD COLUMN active INTEGER NOT NULL DEFAULT 0", [])?;
            conn.execute("ALTER TABLE sessions ADD COLUMN active_pid INTEGER", [])?;
            conn.execute("ALTER TABLE sessions ADD COLUMN active_heartbeat INTEGER", [])?;
        }
        Ok(EventStore { conn })
    }

    pub fn append(&self, event: &Event) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO events (id, parent, branch, session_id, ts, kind, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event.id.to_string(),
                event.parent.map(|p| p.to_string()),
                event.branch.0,
                event.session_id.to_string(),
                event.ts,
                serde_json::to_string(&event.kind)?,
                event
                    .tokens
                    .map(|t| serde_json::to_string(&t))
                    .transpose()?,
            ],
        )?;
        // The cold tier is a STANDING capability: every content-bearing event is
        // indexed here unconditionally, independent of `eviction.enabled`. Gating
        // indexing on the eviction toggle would leave un-searchable gaps in the
        // corpus (turns appended while disabled), so a later re-enable + recall
        // would silently miss them. Recall stays reliable by always indexing.
        if let Some(content) = fts_content(&event.kind) {
            tx.execute(
                "INSERT INTO events_fts (content, event_id, session_id) VALUES (?1, ?2, ?3)",
                params![content, event.id.to_string(), event.session_id.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// BM25-ranked recall over the caller's session content. Returns matching
    /// event ids, best-first, scoped to `session_id` so one session can never
    /// surface another session's (or another repo's) indexed content. The
    /// query is passed to FTS5 wrapped in double quotes so a raw user string
    /// can't be interpreted as FTS syntax (quotes inside are escaped).
    pub fn search_fts(&self, query: &str, session_id: Ulid, limit: usize) -> Result<Vec<Ulid>> {
        let safe = format!("\"{}\"", query.replace('"', "\"\""));
        let mut stmt = self.conn.prepare(
            "SELECT event_id FROM events_fts WHERE events_fts MATCH ?1 AND session_id = ?2 ORDER BY rank LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![safe, session_id.to_string(), limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?.parse()?);
        }
        Ok(out)
    }

    /// Load full events for `ids`, in append (rowid) order, scoped to `session_id`.
    /// Ids not present (or belonging to another session) are skipped. The session
    /// filter is defense-in-depth: `search_fts` already returns only same-session
    /// ids, but scoping here too means a stray id from any other source can never
    /// load a foreign session's event.
    pub fn events_by_ids(&self, ids: &[Ulid], session_id: Ulid) -> Result<Vec<Event>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "{} WHERE id IN ({placeholders}) AND session_id = ? ORDER BY rowid ASC",
            Self::SELECT_COLS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        params.push(session_id.to_string());
        Self::decode_rows(&mut stmt, rusqlite::params_from_iter(params.iter()))
    }

    const SELECT_COLS: &str = "SELECT id, parent, branch, session_id, ts, kind, tokens FROM events";

    fn decode_rows(
        stmt: &mut rusqlite::Statement,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Event>> {
        let raw = stmt.query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in raw {
            let (id, parent, branch, session_id, ts, kind, tokens) = r?;
            out.push(Event {
                id: id.parse()?,
                parent: parent.map(|p| p.parse()).transpose()?,
                branch: BranchId(branch),
                session_id: session_id.parse()?,
                ts,
                kind: serde_json::from_str(&kind)?,
                tokens: tokens.map(|t| serde_json::from_str(&t)).transpose()?,
            });
        }
        Ok(out)
    }

    pub fn load_all(&self) -> Result<Vec<Event>> {
        // append order, not ULID order, so same-ms events replay deterministically
        let mut stmt = self
            .conn
            .prepare(&format!("{} ORDER BY rowid ASC", Self::SELECT_COLS))?;
        Self::decode_rows(&mut stmt, [])
    }

    pub fn load_session(&self, session_id: Ulid) -> Result<Vec<Event>> {
        // append order, not ULID order, so same-ms events replay deterministically
        let mut stmt = self.conn.prepare(&format!(
            "{} WHERE session_id = ?1 ORDER BY rowid ASC",
            Self::SELECT_COLS
        ))?;
        Self::decode_rows(&mut stmt, params![session_id.to_string()])
    }

    pub fn insert_session(
        &self,
        id: Ulid,
        name: &str,
        root_path: &str,
        created_ts: i64,
        last_touched_ts: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (id, name, root_path, created_ts, last_touched_ts)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.to_string(), name, root_path, created_ts, last_touched_ts],
        )?;
        Ok(())
    }

    pub fn rename_session(&self, id: Ulid, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET name = ?2 WHERE id = ?1",
            params![id.to_string(), name],
        )?;
        Ok(())
    }

    pub fn touch_session(&self, id: Ulid, last_touched_ts: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET last_touched_ts = ?2 WHERE id = ?1",
            params![id.to_string(), last_touched_ts],
        )?;
        Ok(())
    }

    /// Persist the active mode name for a session (per-session state, spec §11).
    pub fn set_active_mode(&self, id: Ulid, mode: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET active_mode = ?1 WHERE id = ?2",
            params![mode, id.to_string()],
        )?;
        Ok(())
    }

    /// Claim or clear a session's "active interface" liveness flag (spec §2.2).
    /// `active=true` with `active_pid`/`active_heartbeat` claims the row for
    /// this process; `active=false` (with NULL pid/heartbeat) releases it.
    /// Overwrites any prior claim — this is also the reclaim path (a stale
    /// flag from a crashed process) and the takeover path (another process
    /// overwrites the row, which the old process detects via `heartbeat`).
    pub fn set_active(
        &self,
        id: Ulid,
        active: bool,
        active_pid: i64,
        active_heartbeat: i64,
    ) -> Result<()> {
        if active {
            self.conn.execute(
                "UPDATE sessions SET active = 1, active_pid = ?2, active_heartbeat = ?3 WHERE id = ?1",
                params![id.to_string(), active_pid, active_heartbeat],
            )?;
        } else {
            self.conn.execute(
                "UPDATE sessions SET active = 0, active_pid = NULL, active_heartbeat = NULL WHERE id = ?1",
                params![id.to_string()],
            )?;
        }
        Ok(())
    }

    /// The stored active mode for a session, or `None` if never set.
    pub fn get_active_mode(&self, id: Ulid) -> Result<Option<String>> {
        let v = self
            .conn
            .query_row(
                "SELECT active_mode FROM sessions WHERE id = ?1",
                params![id.to_string()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(v)
    }

    pub fn list_session_rows(&self) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, root_path, created_ts, last_touched_ts, active, active_pid, active_heartbeat FROM sessions ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, root_path, created_ts, last_touched_ts, active, active_pid, active_heartbeat) = r?;
            out.push(SessionRow {
                id: id.parse()?,
                name,
                root_path,
                created_ts,
                last_touched_ts,
                active,
                active_pid,
                active_heartbeat,
            });
        }
        Ok(out)
    }
}

/// Read events from a PRE-session_id legacy DB (6-column `events` schema, no
/// `session_id`). Used once by the bin's one-time import. Returns events with
/// the NIL session sentinel; the caller re-tags them with the target session.
pub fn load_legacy_events(path: &str) -> Result<Vec<Event>> {
    let conn = Connection::open(path)?;
    // append order, not ULID order, so same-ms events replay deterministically
    let mut stmt =
        conn.prepare("SELECT id, parent, branch, ts, kind, tokens FROM events ORDER BY rowid ASC")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut events = Vec::with_capacity(rows.len());
    for (id, parent, branch, ts, kind, tokens) in rows {
        events.push(Event {
            id: id.parse()?,
            parent: parent.map(|p| p.parse()).transpose()?,
            branch: BranchId(branch),
            session_id: Ulid::from(0u128),
            ts,
            kind: serde_json::from_str(&kind)?,
            tokens: tokens.map(|t| serde_json::from_str(&t)).transpose()?,
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use ulid::Ulid;

    #[test]
    fn append_then_load_round_trips_in_order() {
        let store = EventStore::open(":memory:").unwrap();
        let e1 = Event::new(
            Ulid::from(1u128),
            None,
            10,
            EventKind::UserMessage { text: "q".into() },
        );
        let e2 = Event::new(
            Ulid::from(2u128),
            Some(Ulid::from(1u128)),
            20,
            EventKind::AssistantMessage { text: "a".into() },
        );
        store.append(&e1).unwrap();
        store.append(&e2).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded, vec![e1, e2]);
    }

    #[test]
    fn append_persists_session_id_and_load_session_filters() {
        let store = EventStore::open(":memory:").unwrap();
        let sa = Ulid::from(10u128);
        let sb = Ulid::from(20u128);
        let a = Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "a".into() },
        )
        .with_session(sa);
        let b = Event::new(
            Ulid::from(2u128),
            None,
            2,
            EventKind::UserMessage { text: "b".into() },
        )
        .with_session(sb);
        store.append(&a).unwrap();
        store.append(&b).unwrap();
        // load_all keeps every event, with session_id intact …
        assert_eq!(store.load_all().unwrap(), vec![a.clone(), b.clone()]);
        // … load_session partitions the log.
        assert_eq!(store.load_session(sa).unwrap(), vec![a]);
        assert_eq!(store.load_session(sb).unwrap(), vec![b]);
    }

    #[test]
    fn sessions_crud_round_trips() {
        use crate::sessions::SessionRow;
        let store = EventStore::open(":memory:").unwrap();
        let id = Ulid::from(1u128);
        store
            .insert_session(id, "first", "/repo/a", 100, 100)
            .unwrap();
        store.touch_session(id, 200).unwrap();
        store.rename_session(id, "renamed").unwrap();
        let rows = store.list_session_rows().unwrap();
        assert_eq!(
            rows,
            vec![SessionRow {
                id,
                name: "renamed".into(),
                root_path: "/repo/a".into(),
                created_ts: 100,
                last_touched_ts: 200,
                active: false,
                active_pid: None,
                active_heartbeat: None,
            }]
        );
    }

    #[test]
    fn load_all_returns_append_order_not_ulid_order() {
        // ULIDs are DESCENDING as we append (3, 2, 1), so ULID-sort order
        // (1, 2, 3) differs from append order (3, 2, 1). load_all must return
        // events in append order (via rowid), proving it does not rely on the
        // lexicographic ULID ordering.
        let store = EventStore::open(":memory:").unwrap();
        let e3 = Event::new(
            Ulid::from(3u128),
            None,
            10,
            EventKind::UserMessage {
                text: "first-appended".into(),
            },
        );
        let e2 = Event::new(
            Ulid::from(2u128),
            None,
            20,
            EventKind::UserMessage {
                text: "second-appended".into(),
            },
        );
        let e1 = Event::new(
            Ulid::from(1u128),
            None,
            30,
            EventKind::UserMessage {
                text: "third-appended".into(),
            },
        );
        store.append(&e3).unwrap();
        store.append(&e2).unwrap();
        store.append(&e1).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(
            loaded,
            vec![e3, e2, e1],
            "expected append order (3, 2, 1), not ULID order (1, 2, 3)"
        );
    }

    #[test]
    fn load_legacy_events_decodes_old_6_column_schema() {
        use crate::event::EventKind;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE events (
                id     TEXT PRIMARY KEY,
                parent TEXT,
                branch TEXT NOT NULL,
                ts     INTEGER NOT NULL,
                kind   TEXT NOT NULL,
                tokens TEXT
            );",
        )
        .unwrap();

        let id1 = Ulid::from(1u128);
        let id2 = Ulid::from(2u128);
        let kind1 = serde_json::to_string(&EventKind::UserMessage {
            text: "old q".into(),
        })
        .unwrap();
        let kind2 = serde_json::to_string(&EventKind::AssistantMessage {
            text: "old a".into(),
        })
        .unwrap();
        conn.execute(
            "INSERT INTO events (id, parent, branch, ts, kind, tokens) VALUES (?1, NULL, ?2, ?3, ?4, NULL)",
            params![id1.to_string(), "main", 10i64, kind1],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (id, parent, branch, ts, kind, tokens) VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![id2.to_string(), id1.to_string(), "main", 20i64, kind2],
        )
        .unwrap();
        drop(conn);

        let events = load_legacy_events(path.to_str().unwrap()).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, id1);
        assert_eq!(events[0].parent, None);
        assert_eq!(events[0].session_id, Ulid::from(0u128));
        assert_eq!(events[0].tokens, None);
        assert!(matches!(&events[0].kind, EventKind::UserMessage { text } if text == "old q"));
        assert_eq!(events[1].parent, Some(id1));
        assert!(matches!(&events[1].kind, EventKind::AssistantMessage { text } if text == "old a"));
    }

    #[test]
    fn open_creates_secrets_table() {
        let s = EventStore::open(":memory:").unwrap();
        // If the table exists this query succeeds (0 rows); otherwise it errors.
        let n: i64 = s
            .conn
            .query_row("SELECT count(*) FROM secrets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn open_creates_event_embeddings_table() {
        let s = EventStore::open(":memory:").unwrap();
        // If the table exists this query succeeds (0 rows); otherwise it errors.
        let n: i64 = s
            .conn
            .query_row("SELECT count(*) FROM event_embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn append_indexes_searchable_content() {
        let store = EventStore::open(":memory:").unwrap();
        let e = Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "how do I configure the ceiling".into(),
            },
        );
        store.append(&e).unwrap();
        let hits = store.search_fts("ceiling", Ulid::from(0u128), 10).unwrap();
        assert_eq!(hits, vec![Ulid::from(1u128)]);
    }

    #[test]
    fn search_ranks_and_loads_events() {
        let store = EventStore::open(":memory:").unwrap();
        store
            .append(&Event::new(
                Ulid::from(1u128),
                None,
                1,
                EventKind::UserMessage {
                    text: "database indexing strategy".into(),
                },
            ))
            .unwrap();
        store
            .append(&Event::new(
                Ulid::from(2u128),
                None,
                2,
                EventKind::UserMessage {
                    text: "unrelated small talk".into(),
                },
            ))
            .unwrap();
        let ids = store.search_fts("indexing", Ulid::from(0u128), 10).unwrap();
        assert_eq!(ids, vec![Ulid::from(1u128)]);
        let evs = store.events_by_ids(&ids, Ulid::from(0u128)).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].id, Ulid::from(1u128));
    }

    #[test]
    fn search_bad_query_does_not_panic() {
        let store = EventStore::open(":memory:").unwrap();
        // FTS5 special chars must not blow up recall.
        let sid = Ulid::from(0u128);
        assert!(
            store.search_fts("\"unbalanced", sid, 5).is_ok()
                || store.search_fts("\"unbalanced", sid, 5).is_err()
        );
    }

    #[test]
    fn search_fts_ranks_denser_match_first() {
        let store = EventStore::open(":memory:").unwrap();
        let sid = Ulid::from(0u128);
        // Weaker match: the term appears once amid a lot of unrelated padding.
        store
            .append(
                &Event::new(
                    Ulid::from(1u128),
                    None,
                    1,
                    EventKind::UserMessage {
                        text: "widget appears here once amid other unrelated padding content for length".into(),
                    },
                )
                .with_session(sid),
            )
            .unwrap();
        // Stronger match: the term repeated densely in a short document.
        store
            .append(
                &Event::new(
                    Ulid::from(2u128),
                    None,
                    2,
                    EventKind::UserMessage {
                        text: "widget widget widget".into(),
                    },
                )
                .with_session(sid),
            )
            .unwrap();
        let ids = store.search_fts("widget", sid, 10).unwrap();
        assert_eq!(
            ids,
            vec![Ulid::from(2u128), Ulid::from(1u128)],
            "denser match must rank first (best-first BM25 order)"
        );
    }

    #[test]
    fn search_fts_is_session_scoped() {
        let store = EventStore::open(":memory:").unwrap();
        let sa = Ulid::from(100u128);
        let sb = Ulid::from(200u128);
        store
            .append(
                &Event::new(
                    Ulid::from(1u128),
                    None,
                    1,
                    EventKind::UserMessage {
                        text: "shared secret token".into(),
                    },
                )
                .with_session(sa),
            )
            .unwrap();
        store
            .append(
                &Event::new(
                    Ulid::from(2u128),
                    None,
                    2,
                    EventKind::UserMessage {
                        text: "shared secret token".into(),
                    },
                )
                .with_session(sb),
            )
            .unwrap();
        assert_eq!(
            store.search_fts("secret", sa, 10).unwrap(),
            vec![Ulid::from(1u128)]
        );
        assert_eq!(
            store.search_fts("secret", sb, 10).unwrap(),
            vec![Ulid::from(2u128)]
        );
    }

    #[test]
    fn active_mode_round_trips_and_defaults_none() {
        let store = EventStore::open(":memory:").unwrap();
        let id = Ulid::new();
        store.insert_session(id, "s", "/repo", 1, 1).unwrap();
        assert_eq!(store.get_active_mode(id).unwrap(), None); // fresh session
        store.set_active_mode(id, "Superpowers").unwrap();
        assert_eq!(
            store.get_active_mode(id).unwrap(),
            Some("Superpowers".to_string())
        );
    }

    #[test]
    fn migration_is_idempotent_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        // First open creates the column; second open must NOT error on re-migrate.
        {
            let s = EventStore::open(p).unwrap();
            let id = Ulid::new();
            s.insert_session(id, "s", "/r", 1, 1).unwrap();
            s.set_active_mode(id, "M").unwrap();
        }
        let s2 = EventStore::open(p).unwrap(); // re-open: column already exists
                                               // A value written before still reads back.
        let rows = s2.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
        // The persisted active_mode value survives the reopen (not just the row).
        let id: Ulid = rows[0].id; // SessionRow.id is Ulid (Copy) — no .parse()
        assert_eq!(s2.get_active_mode(id).unwrap(), Some("M".to_string()));
    }

    #[test]
    fn migrates_an_old_shape_db_without_active_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let p = path.to_str().unwrap();
        // Simulate a pre-slice-3 DB: sessions table WITHOUT active_mode.
        {
            let conn = rusqlite::Connection::open(p).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, name TEXT NOT NULL, \
                 root_path TEXT NOT NULL, created_ts INTEGER NOT NULL, last_touched_ts INTEGER NOT NULL);"
            ).unwrap();
            conn.execute(
                "INSERT INTO sessions (id,name,root_path,created_ts,last_touched_ts) VALUES (?1,'s','/r',1,1)",
                rusqlite::params![Ulid::new().to_string()],
            ).unwrap();
        }
        // Opening must add the column (not throw) and reads default to None.
        let store = EventStore::open(p).unwrap();
        let rows = store.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
        let id: Ulid = rows[0].id;
        assert_eq!(store.get_active_mode(id).unwrap(), None);
    }

    #[test]
    fn open_migrates_active_liveness_columns_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        // First open adds the columns.
        {
            let s = EventStore::open(p).unwrap();
            let id = Ulid::new();
            s.insert_session(id, "s", "/r", 1, 1).unwrap();
            s.set_active(id, true, 12345, 1000).unwrap();
        }
        // Re-open: the columns already exist; re-migration must NOT error.
        let s2 = EventStore::open(p).unwrap();
        let rows = s2.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].active, true);
        assert_eq!(rows[0].active_pid, Some(12345));
        assert_eq!(rows[0].active_heartbeat, Some(1000));
    }

    #[test]
    fn migrates_an_old_shape_db_without_active_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        let p = path.to_str().unwrap();
        // Simulate a pre-liveness DB: sessions WITHOUT the three columns.
        {
            let conn = rusqlite::Connection::open(p).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, name TEXT NOT NULL, \
                 root_path TEXT NOT NULL, created_ts INTEGER NOT NULL, last_touched_ts INTEGER NOT NULL, \
                 active_mode TEXT);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (id,name,root_path,created_ts,last_touched_ts) VALUES (?1,'s','/r',1,1)",
                rusqlite::params![Ulid::new().to_string()],
            )
            .unwrap();
        }
        // Opening must add the columns (not throw) and they default to inactive.
        let s = EventStore::open(p).unwrap();
        let rows = s.list_session_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].active, false);
        assert_eq!(rows[0].active_pid, None);
        assert_eq!(rows[0].active_heartbeat, None);
    }

    #[test]
    fn fts5_virtual_table_is_available() {
        let store = EventStore::open(":memory:").unwrap();
        // If FTS5 is compiled in, this query against the events_fts table succeeds.
        let n: i64 = store
            .conn
            .query_row("SELECT count(*) FROM events_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn open_sets_wal_journal_mode_and_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        let s = EventStore::open(p).unwrap();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let timeout: i64 = s
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    #[test]
    fn wal_mode_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.db");
        let p = path.to_str().unwrap();
        {
            let _s = EventStore::open(p).unwrap();
        }
        let s2 = EventStore::open(p).unwrap();
        let mode: String = s2
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }
}
