//! `KillSlot` — a single-slot registry the `shell` tool publishes its running
//! child's process-group id into, so the async agent loop can SIGKILL the whole
//! group on a hard-stop. One slot suffices: Local tools in a batch run
//! sequentially, so at most one shell child exists at a time. `kill()` is
//! sticky: if it races ahead of the child's `register()`, the child is killed
//! the instant it registers (no lost-wakeup window).

use std::sync::{Arc, Mutex};

#[derive(Debug, Default)]
struct Inner {
    pgid: Option<u32>,
    kill_requested: bool,
}

/// Shared handle to the currently-running killable child's process group.
#[derive(Debug, Clone, Default)]
pub struct KillSlot(Arc<Mutex<Inner>>);

impl KillSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the pgid of a freshly-spawned killable child. If a hard-stop was
    /// already requested (kill() raced ahead of the spawn), signal the child
    /// immediately instead of storing it.
    pub fn register(&self, pgid: u32) {
        let mut g = self.0.lock().unwrap();
        if g.kill_requested {
            drop(g); // release the lock before the syscall
            Self::signal(pgid);
        } else {
            g.pgid = Some(pgid);
        }
    }

    /// Forget the child (called when it exits normally).
    pub fn clear(&self) {
        self.0.lock().unwrap().pgid = None;
    }

    /// The currently-registered pgid, if any (test/introspection).
    pub fn pgid(&self) -> Option<u32> {
        self.0.lock().unwrap().pgid
    }

    /// Request a hard-stop: arm the sticky flag and SIGKILL the registered
    /// group now (if any). A child that registers a moment later is killed by
    /// `register()`. Best-effort; ignores errors (e.g. `ESRCH`).
    pub fn kill(&self) {
        let pgid = {
            let mut g = self.0.lock().unwrap();
            g.kill_requested = true;
            g.pgid
        }; // lock dropped before the syscall
        if let Some(pgid) = pgid {
            Self::signal(pgid);
        }
    }

    /// SIGKILL the whole process group: `killpg` targets the pgid directly
    /// (no negation, unlike `kill(2)` on a group). Unix-only; no-op else.
    #[cfg(unix)]
    fn signal(pgid: u32) {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        let _ = killpg(Pid::from_raw(pgid as i32), Signal::SIGKILL);
    }

    #[cfg(not(unix))]
    fn signal(_pgid: u32) {
        // No process-group signalling on non-unix; hard-stop degrades to
        // abandon (the shell command finishes on its own).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_pgid_then_clear() {
        let slot = KillSlot::new();
        assert_eq!(slot.pgid(), None);
        slot.register(4242);
        assert_eq!(slot.pgid(), Some(4242));
        slot.clear();
        assert_eq!(slot.pgid(), None);
    }

    #[test]
    fn kill_on_empty_slot_is_noop() {
        let slot = KillSlot::new();
        slot.kill(); // must not panic when nothing is registered
        assert_eq!(slot.pgid(), None);
    }

    // The kill-before-register race: kill() arrives first, then a real child
    // registers. It must be signalled immediately. Proven with a real process
    // spawned in its own group that would outlive the test if not killed.
    #[cfg(unix)]
    #[test]
    fn kill_before_register_still_terminates_the_child() {
        use std::os::unix::process::CommandExt;
        use std::process::Command;
        use std::time::Duration;
        let slot = KillSlot::new();
        slot.kill(); // hard-stop raced ahead of the spawn
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .process_group(0)
            .spawn()
            .unwrap();
        slot.register(child.id()); // must kill immediately, not store
        // The child should die within moments (SIGKILL), well under its 30s.
        let mut waited = 0;
        loop {
            match child.try_wait().unwrap() {
                Some(_status) => break, // reaped — it was killed
                None if waited < 2000 => {
                    std::thread::sleep(Duration::from_millis(20));
                    waited += 20;
                }
                None => panic!("child survived a pre-registered kill request"),
            }
        }
    }
}
