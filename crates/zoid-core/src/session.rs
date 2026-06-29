use crate::event::Event;
use crate::store::EventStore;
use anyhow::Result;
use tokio::sync::{mpsc, oneshot};

/// Commands accepted by the single-writer store actor.
enum Cmd {
    Append { event: Box<Event>, reply: oneshot::Sender<Result<()>> },
    Snapshot { reply: oneshot::Sender<Vec<Event>> },
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
                        let snap = store.load_all().unwrap_or_default();
                        let _ = reply.send(snap);
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
            .send(Cmd::Append { event: Box::new(event), reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))?
    }

    /// An ordered, immutable snapshot of the full log.
    pub async fn snapshot(&self) -> Result<Vec<Event>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Snapshot { reply })
            .await
            .map_err(|_| anyhow::anyhow!("session actor stopped"))?;
        rx.await.map_err(|_| anyhow::anyhow!("session actor dropped reply"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use ulid::Ulid;

    #[tokio::test]
    async fn appends_then_snapshots_in_order() {
        let handle = SessionHandle::spawn(":memory:").unwrap();
        let e1 = Event::new(Ulid::from(1u128), None, 10, EventKind::UserMessage { text: "q".into() });
        let e2 = Event::new(Ulid::from(2u128), Some(Ulid::from(1u128)), 20,
            EventKind::ModelDelta { text: "a".into() });
        handle.append(e1.clone()).await.unwrap();
        handle.append(e2.clone()).await.unwrap();

        let snap = handle.snapshot().await.unwrap();
        assert_eq!(snap, vec![e1, e2]);
    }

    #[tokio::test]
    async fn clone_shares_the_same_actor() {
        let handle = SessionHandle::spawn(":memory:").unwrap();
        let e1 = Event::new(Ulid::from(1u128), None, 1, EventKind::UserMessage { text: "x".into() });
        handle.clone().append(e1.clone()).await.unwrap();
        let snap = handle.snapshot().await.unwrap();
        assert_eq!(snap, vec![e1]);
    }
}
