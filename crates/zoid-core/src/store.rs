use crate::event::{BranchId, Event};
use crate::sessions::SessionRow;
use anyhow::Result;
use rusqlite::{params, Connection};
use ulid::Ulid;

/// Single-writer, append-only event log backed by SQLite. The store owns the
/// connection; readers obtain owned `Vec<Event>` snapshots via `load_all`.
pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
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
            );",
        )?;
        Ok(EventStore { conn })
    }

    pub fn append(&self, event: &Event) -> Result<()> {
        self.conn.execute(
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
        Ok(())
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

    pub fn list_session_rows(&self) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, root_path, created_ts, last_touched_ts FROM sessions ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (id, name, root_path, created_ts, last_touched_ts) = r?;
            out.push(SessionRow {
                id: id.parse()?,
                name,
                root_path,
                created_ts,
                last_touched_ts,
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
}
