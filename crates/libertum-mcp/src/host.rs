//! How the server reaches the application.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

use crate::protocol::{ClientInfo, HostError, HostResult, Request};

/// Whatever answers the server's questions. The application implements this; the tests use a
/// fake. `request` may block (the server calls it off its async threads).
pub trait Host: Send + Sync + 'static {
    fn request(&self, client: &ClientInfo, request: Request) -> HostResult;
}

const WAITING: u8 = 0;
const CLAIMED: u8 = 1;
const ABANDONED: u8 = 2;

/// How long the server keeps waiting once the application has started on an edit, even though
/// the normal wait is over.
const CLAIMED_GRACE: Duration = Duration::from_secs(10);

/// A question waiting for the application's thread to answer it.
pub struct Pending {
    client: ClientInfo,
    request: Request,
    reply: SyncSender<HostResult>,
    state: Arc<AtomicU8>,
}

impl Pending {
    pub fn client(&self) -> &ClientInfo {
        &self.client
    }

    pub fn request(&self) -> &Request {
        &self.request
    }

    /// Whether the server has already told the client this request failed. An edit that is
    /// still waiting for the writer's approval can reach this point; it must not be applied.
    pub fn is_abandoned(&self) -> bool {
        self.state.load(Ordering::Acquire) == ABANDONED
    }

    /// Call this immediately before acting on an edit. `true` means the server will wait for
    /// the answer, so go ahead; `false` means the client was already told the request timed
    /// out, so the edit must not be made. This closes the gap between checking and acting.
    pub fn claim(&self) -> bool {
        self.state
            .compare_exchange(WAITING, CLAIMED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Send the answer back. The server may have stopped waiting; that is not an error.
    pub fn respond(self, result: HostResult) {
        let _ = self.reply.send(result);
    }
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("client", &self.client)
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

/// A [`Host`] that forwards every request over a channel to the application's own thread and
/// waits for the answer: up to `read_timeout` for a read, and up to `edit_timeout` for an edit
/// (which may be waiting for the writer to approve it).
pub struct ChannelHost {
    sender: mpsc::Sender<Pending>,
    read_timeout: Duration,
    edit_timeout: Duration,
}

/// Make a channel host and the receiving end the application drains.
pub fn channel(
    read_timeout: Duration,
    edit_timeout: Duration,
) -> (Arc<ChannelHost>, Receiver<Pending>) {
    let (sender, receiver) = mpsc::channel();
    (
        Arc::new(ChannelHost {
            sender,
            read_timeout,
            edit_timeout,
        }),
        receiver,
    )
}

impl Host for ChannelHost {
    fn request(&self, client: &ClientInfo, request: Request) -> HostResult {
        let timeout = if request.is_edit() {
            self.edit_timeout
        } else {
            self.read_timeout
        };
        let (reply, answer) = mpsc::sync_channel(1);
        let state = Arc::new(AtomicU8::new(WAITING));
        self.sender
            .send(Pending {
                client: client.clone(),
                request,
                reply,
                state: Arc::clone(&state),
            })
            .map_err(|_| HostError::Unavailable("the application is not listening".into()))?;
        match answer.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if state
                    .compare_exchange(WAITING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Err(HostError::Unavailable("it took too long to answer".into()));
                }
                // The application is already acting on it: wait for the outcome rather than
                // report a failure for something that is happening.
                match answer.recv_timeout(CLAIMED_GRACE) {
                    Ok(result) => result,
                    Err(_) => Err(HostError::Unavailable(
                        "the edit was started but its outcome is unknown; check the document"
                            .into(),
                    )),
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(HostError::Unavailable("the request was dropped".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Position;

    fn edit() -> Request {
        Request::ReplaceRange {
            document: None,
            start: Position {
                paragraph: 0,
                offset: 0,
            },
            end: Position {
                paragraph: 0,
                offset: 0,
            },
            text: "x".into(),
        }
    }

    fn client() -> ClientInfo {
        ClientInfo::new("test", "1")
    }

    #[test]
    fn an_unanswered_edit_is_abandoned_and_cannot_be_claimed_afterwards() {
        let (host, queue) = channel(Duration::from_millis(50), Duration::from_millis(50));
        let result = host.request(&client(), edit());
        assert!(matches!(result, Err(HostError::Unavailable(_))));
        let pending = queue.try_recv().expect("it was queued");
        assert!(pending.is_abandoned());
        assert!(!pending.claim(), "an abandoned edit must not be applied");
    }

    #[test]
    fn a_claimed_edit_is_waited_for_past_the_timeout() {
        let (host, queue) = channel(Duration::from_millis(50), Duration::from_millis(50));
        let worker = std::thread::spawn(move || {
            let pending = queue.recv().expect("queued");
            assert!(pending.claim());
            std::thread::sleep(Duration::from_millis(200));
            pending.respond(Ok(crate::Reply::Edited {
                document: 1,
                message: "done".into(),
            }));
        });
        let result = host.request(&client(), edit());
        assert!(
            matches!(result, Ok(crate::Reply::Edited { .. })),
            "{result:?}"
        );
        worker.join().expect("worker");
    }

    #[test]
    fn edits_wait_longer_than_reads() {
        let (host, queue) = channel(Duration::from_millis(30), Duration::from_millis(400));
        let worker = std::thread::spawn(move || {
            let pending = queue.recv().expect("queued");
            std::thread::sleep(Duration::from_millis(150));
            assert!(pending.claim());
            pending.respond(Ok(crate::Reply::Edited {
                document: 1,
                message: "done".into(),
            }));
        });
        assert!(host.request(&client(), edit()).is_ok());
        worker.join().expect("worker");

        // A read given the same delay would have timed out.
        let (host, queue) = channel(Duration::from_millis(30), Duration::from_millis(400));
        let worker = std::thread::spawn(move || {
            let pending = queue.recv().expect("queued");
            std::thread::sleep(Duration::from_millis(150));
            pending.respond(Ok(crate::Reply::Documents(Vec::new())));
        });
        assert!(host.request(&client(), Request::ListDocuments).is_err());
        worker.join().expect("worker");
    }
}
