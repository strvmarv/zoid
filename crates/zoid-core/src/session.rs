use crate::event::Event;
use crate::sessions::SessionInfo;
use crate::store::EventStore;
use anyhow::Result;
use tokio::sync::{mpsc, oneshot};
use ulid::Ulid;

/// Commands accepted by the single-writer store actor.
enum Cmd {
    Append {
        event: Box<Event>,
        reply: oneshot::Sender<Result<()>>,
    },
    Snapshot {
        reply: oneshot::Sender<Result<Vec<Event>>>,
    },
    SnapshotSession {
        session_id: Ulid,
        reply: oneshot::Sender<Result<Vec<Event>>>,
    },
    NewSession {
        id: Ulid,
        name: String,
        root_path: String,
        ts: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    RenameSession {
        id: Ulid,
        name: String,
        reply: oneshot::Sender<Result<()>>,
    },
    TouchSession {
        id: Ulid,
        ts: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    ListSessions {
        root_filter: Option<String>,
        reply: oneshot::Sender<Result<Vec<SessionInfo>>>,
    },
    Recall {
        query: String,
        session_id: Ulid,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<Event>>>,
    },
}

/// A cloneable handle to the single-writer event-store actor (spec §4.1).
///
/// The store's `rusqlite::Connection` is blocking, so it lives on a dedicated
/// OS thread that serializes every append/read. Async callers send commands
/// over an `mpsc` channel and await the reply via `oneshot`, so SQLite work
/// never blocks the tokio runtime.
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<Cmd>,
}

impl SessionHandle {
    /// Open the store at `path` and spawn its writer thread. Errors if the
    /// store cannot be opened (the open happens on the caller so the error
    /// surfaces synchronously).
    pub fn spawn(path: &str) -> Result<SessionHandle> {
        let store = EventStore::open(path)?;
        let (tx, mut rx) = mpsc::channel::<Cmd>(64);
        std::thread::spawn(move || {
            // `blocking_recv` is valid here: this thread is not a tokio worker.
            while let Some(cmd) = rx.blocking_recv() {
                match cmd {
                    Cmd::Append { event, reply } => {
                        let _ = reply.send(store.append(&event));
                    }
                    Cmd::Snapshot { reply } => {
                        // Propagate read failures (corruption, disk error) instead
                        // of masking them as an empty log — an empty snapshot and a
                        // failed read are very different to the caller.
                        let _ = reply.send(store.load_all());
                    }
                    Cmd::SnapshotSession { session_id, reply } => {
                        let _ = reply.send(store.load_session(session_id));
                    }
                    Cmd::NewSession {
                        id,
                        name,
                        root_path,
                        ts,
                        reply,
                    } => {
                        let _ = reply.send(store.insert_session(id, &name, &root_path, ts, ts));
                    }
                    Cmd::RenameSession { id, name, reply } => {
                        let _ = reply.send(store.rename_session(id, &name));
                    }
                    Cmd::TouchSession { id, ts, reply } => {
                        let _ = reply.send(store.touch_session(id, ts));
                    }
                    Cmd::ListSessions { root_filter, reply } => {
                        let out = (|| {
                            let rows = store.list_session_rows()?;
                            let events = store.load_all()?;
                            anyhow::Ok(crate::sessions::session_list(
                                &rows,
                                &events,
                                root_filter.as_deref(),
                            ))
                        })();
                        let _ = reply.send(out);
                    }
                    Cmd::Recall {
                        query,
                        session_id,
                        limit,
                        reply,
                    } => {
                        let out = store
                            .search_fts(&query, session_id, limit)
                            .and_then(|ids| store.events_by_ids(&ids));
                        let _ = reply.send(out);
                    }
                }
            }
        });
        Ok(SessionHandle { tx })
    }

    /// Durably append one event. Awaits the actor's confirmation.
    pub async fn append(&self, event: Event) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Append {
                event: Box::new(event),
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// An ordered, immutable snapshot of the full log.
    pub async fn snapshot(&self) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Snapshot { reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// An ordered, immutable snapshot of one session's events.
    pub async fn snapshot_session(&self, session_id: Ulid) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::SnapshotSession { session_id, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Create a new session row.
    pub async fn new_session(
        &self,
        id: Ulid,
        name: String,
        root_path: String,
        ts: i64,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::NewSession {
                id,
                name,
                root_path,
                ts,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Rename an existing session.
    pub async fn rename_session(&self, id: Ulid, name: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::RenameSession { id, name, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Bump a session's `last_touched_ts`.
    pub async fn touch_session(&self, id: Ulid, ts: i64) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::TouchSession { id, ts, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// List sessions (optionally filtered by root path) for the resume picker.
    pub async fn list_sessions(&self, root_filter: Option<String>) -> Result<Vec<SessionInfo>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::ListSessions { root_filter, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Search the cold tier (BM25 via FTS5), scoped to `session_id`, and load
    /// matching events, best-first.
    pub async fn recall(&self, query: String, session_id: Ulid, limit: usize) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Recall {
                query,
                session_id,
                limit,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;

    #[tokio::test]
    async fn appends_then_snapshots_in_order() {
        let handle = SessionHandle::spawn(":memory:").unwrap();
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
            EventKind::ModelDelta { text: "a".into() },
        );
        handle.append(e1.clone()).await.unwrap();
        handle.append(e2.clone()).await.unwrap();

        let snap = handle.snapshot().await.unwrap();
        assert_eq!(snap, vec![e1, e2]);
    }

    #[tokio::test]
    async fn clone_shares_the_same_actor() {
        let handle = SessionHandle::spawn(":memory:").unwrap();
        let e1 = Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage { text: "x".into() },
        );
        handle.clone().append(e1.clone()).await.unwrap();
        let snap = handle.snapshot().await.unwrap();
        assert_eq!(snap, vec![e1]);
    }

    #[tokio::test]
    async fn actor_partitions_sessions_and_lists_them() {
        let handle = SessionHandle::spawn(":memory:").unwrap();
        let sa = Ulid::from(1u128);
        handle
            .new_session(sa, "alpha".into(), "/repo".into(), 100)
            .await
            .unwrap();
        handle
            .append(
                Event::new(
                    Ulid::from(9u128),
                    None,
                    5,
                    EventKind::UserMessage { text: "hi".into() },
                )
                .with_session(sa),
            )
            .await
            .unwrap();
        // session-scoped snapshot returns only sa's events
        let snap = handle.snapshot_session(sa).await.unwrap();
        assert_eq!(snap.len(), 1);
        // list surfaces the row
        let list = handle.list_sessions(Some("/repo".into())).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "alpha");
    }

    #[tokio::test]
    async fn recall_finds_indexed_events() {
        let h = SessionHandle::spawn(":memory:").unwrap();
        h.append(Event::new(
            Ulid::from(1u128),
            None,
            1,
            EventKind::UserMessage {
                text: "vector search backend".into(),
            },
        ))
        .await
        .unwrap();
        h.append(Event::new(
            Ulid::from(2u128),
            None,
            2,
            EventKind::UserMessage {
                text: "hello".into(),
            },
        ))
        .await
        .unwrap();
        let hits = h.recall("vector".into(), Ulid::from(0u128), 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Ulid::from(1u128));
    }
}
