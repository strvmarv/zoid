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
    SetActiveMode {
        id: Ulid,
        mode: String,
        reply: oneshot::Sender<Result<()>>,
    },
    GetActiveMode {
        id: Ulid,
        reply: oneshot::Sender<Result<Option<String>>>,
    },
    SetActive {
        id: Ulid,
        active: bool,
        active_pid: i64,
        active_heartbeat: i64,
        reply: oneshot::Sender<Result<()>>,
    },
    Heartbeat {
        id: Ulid,
        active_pid: i64,
        active_heartbeat: i64,
        reply: oneshot::Sender<Result<bool>>,
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
    WriteEmbedding {
        event_id: Ulid,
        model_id: String,
        vector: Vec<f32>,
        reply: oneshot::Sender<Result<()>>,
    },
    LoadRecentEmbeddings {
        model_id: String,
        cap: usize,
        reply: oneshot::Sender<Result<Vec<(Ulid, Vec<f32>)>>>,
    },
    UnembeddedEvents {
        model_id: String,
        session_id: Ulid,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<(Ulid, String)>>>,
    },
    UnembeddedEventsAll {
        model_id: String,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<(Ulid, String)>>>,
    },
    EventsByIds {
        ids: Vec<Ulid>,
        session_id: Ulid,
        reply: oneshot::Sender<Result<Vec<Event>>>,
    },
    DeleteSession {
        id: Ulid,
        reply: oneshot::Sender<Result<()>>,
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
                    Cmd::DeleteSession { id, reply } => {
                        let _ = reply.send(store.delete_session(id));
                    }
                    Cmd::SetActiveMode { id, mode, reply } => {
                        let _ = reply.send(store.set_active_mode(id, &mode));
                    }
                    Cmd::GetActiveMode { id, reply } => {
                        let _ = reply.send(store.get_active_mode(id));
                    }
                    Cmd::SetActive {
                        id,
                        active,
                        active_pid,
                        active_heartbeat,
                        reply,
                    } => {
                        let _ =
                            reply.send(store.set_active(id, active, active_pid, active_heartbeat));
                    }
                    Cmd::Heartbeat {
                        id,
                        active_pid,
                        active_heartbeat,
                        reply,
                    } => {
                        let _ = reply.send(store.heartbeat(id, active_pid, active_heartbeat));
                    }
                    Cmd::ListSessions { root_filter, reply } => {
                        let out = (|| {
                            let rows = store.list_session_rows()?;
                            let totals = store.session_token_totals()?;
                            anyhow::Ok(crate::sessions::session_list(
                                &rows,
                                &totals,
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
                            .and_then(|ids| store.events_by_ids(&ids, session_id));
                        let _ = reply.send(out);
                    }
                    Cmd::WriteEmbedding {
                        event_id,
                        model_id,
                        vector,
                        reply,
                    } => {
                        let _ = reply.send(store.write_embedding(event_id, &model_id, &vector));
                    }
                    Cmd::LoadRecentEmbeddings {
                        model_id,
                        cap,
                        reply,
                    } => {
                        let _ = reply.send(store.load_recent_embeddings(&model_id, cap));
                    }
                    Cmd::UnembeddedEvents {
                        model_id,
                        session_id,
                        limit,
                        reply,
                    } => {
                        let _ = reply.send(store.unembedded_events(&model_id, session_id, limit));
                    }
                    Cmd::UnembeddedEventsAll {
                        model_id,
                        limit,
                        reply,
                    } => {
                        let _ = reply.send(store.unembedded_events_all(&model_id, limit));
                    }
                    Cmd::EventsByIds {
                        ids,
                        session_id,
                        reply,
                    } => {
                        let _ = reply.send(store.events_by_ids(&ids, session_id));
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

    /// Delete a session and all its events/FTS/embeddings. Only call this for
    /// non-live sessions (the heartbeat invariant relies on live sessions
    /// never being deleted).
    pub async fn delete_session(&self, id: Ulid) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::DeleteSession { id, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Persist the active mode for a session.
    pub async fn set_active_mode(&self, id: Ulid, mode: String) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::SetActiveMode { id, mode, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Read the stored active mode for a session (None if never set).
    pub async fn get_active_mode(&self, id: Ulid) -> Result<Option<String>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::GetActiveMode { id, reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Claim or release the active-interface flag for a session (spec §2.2).
    pub async fn set_active(
        &self,
        id: Ulid,
        active: bool,
        active_pid: i64,
        active_heartbeat: i64,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::SetActive {
                id,
                active,
                active_pid,
                active_heartbeat,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Refresh the heartbeat for the current session. Returns `false` when
    /// another process has taken over (the yield signal). Spec §2.3.
    pub async fn heartbeat(
        &self,
        id: Ulid,
        active_pid: i64,
        active_heartbeat: i64,
    ) -> Result<bool> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Heartbeat {
                id,
                active_pid,
                active_heartbeat,
                reply,
            })
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
    pub async fn recall(
        &self,
        query: String,
        session_id: Ulid,
        limit: usize,
    ) -> Result<Vec<Event>> {
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

    /// Persist an embedding vector for an event under a given model id.
    pub async fn write_embedding(
        &self,
        event_id: Ulid,
        model_id: String,
        vector: Vec<f32>,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::WriteEmbedding {
                event_id,
                model_id,
                vector,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Load up to `cap` most-recent embeddings for a model id.
    pub async fn load_recent_embeddings(
        &self,
        model_id: String,
        cap: usize,
    ) -> Result<Vec<(Ulid, Vec<f32>)>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::LoadRecentEmbeddings {
                model_id,
                cap,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// List events in a session that have not yet been embedded under a model id.
    pub async fn unembedded_events(
        &self,
        model_id: String,
        session_id: Ulid,
        limit: usize,
    ) -> Result<Vec<(Ulid, String)>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::UnembeddedEvents {
                model_id,
                session_id,
                limit,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Unembedded searchable events across ALL sessions (see
    /// [`crate::store::EventStore::unembedded_events_all`]). Used by the embed
    /// lane so events in any session get embedded, not just the boot session's.
    pub async fn unembedded_events_all(
        &self,
        model_id: String,
        limit: usize,
    ) -> Result<Vec<(Ulid, String)>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::UnembeddedEventsAll {
                model_id,
                limit,
                reply,
            })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// Load events by id, scoped to `session_id`.
    pub async fn events_by_ids(&self, ids: Vec<Ulid>, session_id: Ulid) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::EventsByIds {
                ids,
                session_id,
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
        let hits = h
            .recall("vector".into(), Ulid::from(0u128), 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, Ulid::from(1u128));
    }

    #[tokio::test]
    async fn set_active_and_heartbeat_round_trip_via_actor() {
        let h = SessionHandle::spawn(":memory:").unwrap();
        let id = Ulid::from(7u128);
        h.new_session(id, "s".into(), "/r".into(), 0).await.unwrap();
        h.set_active(id, true, 1234, 1000).await.unwrap();
        // Heartbeat as the owner refreshes the timestamp.
        assert!(h.heartbeat(id, 1234, 2000).await.unwrap());
        // A takeover (overwrite pid) makes the old owner's heartbeat return false.
        h.set_active(id, true, 5678, 1500).await.unwrap();
        assert!(!h.heartbeat(id, 1234, 2500).await.unwrap());
    }

    #[tokio::test]
    async fn handle_embedding_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let h = SessionHandle::spawn(dir.path().join("s.db").to_str().unwrap()).unwrap();
        let sid = Ulid::from(1u128);
        h.new_session(sid, "s".into(), "/tmp".into(), 0)
            .await
            .unwrap();
        // 2nd arg is parent, not session — set session via .with_session (event.rs:200).
        let ev = Event::new(
            Ulid::from(10u128),
            None,
            1,
            EventKind::UserMessage {
                text: "hi there".into(),
            },
        )
        .with_session(sid);
        h.append(ev).await.unwrap();

        assert_eq!(
            h.unembedded_events("bge".into(), sid, 10).await.unwrap().len(),
            1
        );
        h.write_embedding(Ulid::from(10u128), "bge".into(), vec![0.5, 0.5])
            .await
            .unwrap();
        assert_eq!(
            h.unembedded_events("bge".into(), sid, 10).await.unwrap().len(),
            0
        );
        let loaded = h.load_recent_embeddings("bge".into(), 10).await.unwrap();
        assert_eq!(loaded, vec![(Ulid::from(10u128), vec![0.5, 0.5])]);
        let evs = h
            .events_by_ids(vec![Ulid::from(10u128)], sid)
            .await
            .unwrap();
        assert_eq!(evs.len(), 1);
    }

    #[tokio::test]
    async fn delete_session_removes_session() {
        let store = SessionHandle::spawn(":memory:").unwrap();
        let sid = Ulid::new();
        store.new_session(sid, "test".into(), "/repo".into(), 0).await.unwrap();
        // Confirm it exists.
        let before = store.list_sessions(None).await.unwrap();
        assert!(before.iter().any(|s| s.id == sid));
        // Delete it.
        store.delete_session(sid).await.unwrap();
        // Confirm it's gone.
        let after = store.list_sessions(None).await.unwrap();
        assert!(!after.iter().any(|s| s.id == sid), "session must be deleted");
    }
}
