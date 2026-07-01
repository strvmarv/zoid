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
                event.tokens.map(|t| serde_json::to_string(&t)).transpose()?,
            ],
        )?;
        Ok(())
    }

    const SELECT_COLS: &str =
        "SELECT id, parent, branch, session_id, ts, kind, tokens FROM events";

    fn decode_rows(stmt: &mut rusqlite::Statement, params: impl rusqlite::Params) -> Result<Vec<Event>> {
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
        let mut stmt = self.conn.prepare(&format!("{} ORDER BY id ASC", Self::SELECT_COLS))?;
        Self::decode_rows(&mut stmt, [])
    }

    pub fn load_session(&self, session_id: Ulid) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(&format!("{} WHERE session_id = ?1 ORDER BY id ASC", Self::SELECT_COLS))?;
        Self::decode_rows(&mut stmt, params![session_id.to_string()])
    }

    pub fn insert_session(&self, id: Ulid, name: &str, root_path: &str, created_ts: i64, last_touched_ts: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (id, name, root_path, created_ts, last_touched_ts)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id.to_string(), name, root_path, created_ts, last_touched_ts],
        )?;
        Ok(())
    }

    pub fn rename_session(&self, id: Ulid, name: &str) -> Result<()> {
        self.conn.execute("UPDATE sessions SET name = ?2 WHERE id = ?1", params![id.to_string(), name])?;
        Ok(())
    }

    pub fn touch_session(&self, id: Ulid, last_touched_ts: i64) -> Result<()> {
        self.conn.execute("UPDATE sessions SET last_touched_ts = ?2 WHERE id = ?1",
            params![id.to_string(), last_touched_ts])?;
        Ok(())
    }

    pub fn list_session_rows(&self) -> Result<Vec<SessionRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, root_path, created_ts, last_touched_ts FROM sessions ORDER BY id ASC")?;
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
            out.push(SessionRow { id: id.parse()?, name, root_path, created_ts, last_touched_ts });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use ulid::Ulid;

    #[test]
    fn append_then_load_round_trips_in_order() {
        let store = EventStore::open(":memory:").unwrap();
        let e1 = Event::new(Ulid::from(1u128), None, 10, EventKind::UserMessage { text: "q".into() });
        let e2 = Event::new(Ulid::from(2u128), Some(Ulid::from(1u128)), 20,
            EventKind::AssistantMessage { text: "a".into() });
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
        let a = Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "a".into() }).with_session(sa);
        let b = Event::new(Ulid::from(2u128), None, 2, EventKind::UserMessage { text: "b".into() }).with_session(sb);
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
        store.insert_session(id, "first", "/repo/a", 100, 100).unwrap();
        store.touch_session(id, 200).unwrap();
        store.rename_session(id, "renamed").unwrap();
        let rows = store.list_session_rows().unwrap();
        assert_eq!(rows, vec![SessionRow {
            id, name: "renamed".into(), root_path: "/repo/a".into(),
            created_ts: 100, last_touched_ts: 200,
        }]);
    }
}
