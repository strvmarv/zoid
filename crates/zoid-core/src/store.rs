use crate::event::{BranchId, Event};
use anyhow::Result;
use rusqlite::{params, Connection};

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
                id     TEXT PRIMARY KEY,
                parent TEXT,
                branch TEXT NOT NULL,
                ts     INTEGER NOT NULL,
                kind   TEXT NOT NULL,
                tokens TEXT
            );",
        )?;
        Ok(EventStore { conn })
    }

    pub fn append(&self, event: &Event) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (id, parent, branch, ts, kind, tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event.id.to_string(),
                event.parent.map(|p| p.to_string()),
                event.branch.0,
                event.ts,
                serde_json::to_string(&event.kind)?,
                event.tokens.map(|t| serde_json::to_string(&t)).transpose()?,
            ],
        )?;
        Ok(())
    }

    pub fn load_all(&self) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, parent, branch, ts, kind, tokens FROM events ORDER BY id ASC",
        )?;
        // The row closure must return rusqlite::Result, so pull raw columns here
        // and do (fallible) serde decoding outside the closure.
        let raw = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;

        let mut out = Vec::new();
        for r in raw {
            let (id, parent, branch, ts, kind, tokens) = r?;
            out.push(Event {
                id: id.parse()?,
                parent: parent.map(|p| p.parse()).transpose()?,
                branch: BranchId(branch),
                ts,
                kind: serde_json::from_str(&kind)?,
                tokens: tokens.map(|t| serde_json::from_str(&t)).transpose()?,
            });
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
}
