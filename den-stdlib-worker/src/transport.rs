//! The channel a `MessagePort` pair — and therefore a `Worker` — is made of.
//!
//! Nothing here touches a `JSValue`: a [`PortHandle`] is plain `Send` Rust data
//! precisely so it can be moved onto a worker thread, or into a [`Message`]
//! when a port is transferred.

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender, error::SendError};

use crate::message::Message;

/// What travels down a port's channel.
#[derive(Debug)]
pub enum Envelope {
    Message(Message),
    /// The peer is gone — `close()`, or the context that owned it was dropped.
    /// A pump that sees this ends; the port is entangled with nothing.
    Close,
}

/// One end of an entangled pair.
///
/// Owns the sender into the *peer's* inbox and, until a pump takes it, its own
/// inbox. Dropping a handle announces the closure to the peer, so a pump on the
/// other side always terminates instead of parking forever.
#[derive(Debug)]
pub struct PortHandle {
    outbox: Option<UnboundedSender<Envelope>>,
    inbox:  Option<UnboundedReceiver<Envelope>>,
}

impl PortHandle {
    /// Two handles, entangled: what one sends, the other receives.
    pub fn pair() -> (Self, Self) {
        let (to_first, first_inbox) = mpsc::unbounded_channel();
        let (to_second, second_inbox) = mpsc::unbounded_channel();
        (
            Self {
                outbox: Some(to_second),
                inbox:  Some(first_inbox),
            },
            Self {
                outbox: Some(to_first),
                inbox:  Some(second_inbox),
            },
        )
    }

    /// Hand `envelope` to the peer. The envelope comes back in the `Err` when
    /// there is no peer left to take it — which is not an error in the spec:
    /// posting to a closed port is silently a no-op.
    pub fn send(&self, envelope: Envelope) -> Result<(), SendError<Envelope>> {
        match self.outbox.as_ref() {
            Some(outbox) => outbox.send(envelope),
            None => Err(SendError(envelope)),
        }
    }

    /// Take this end's inbox, once, for a pump to await. `None` afterwards.
    pub fn take_receiver(&mut self) -> Option<UnboundedReceiver<Envelope>> {
        self.inbox.take()
    }

    /// Whether this end can still deliver to its peer.
    pub fn is_open(&self) -> bool {
        self.outbox.is_some()
    }

    /// Detach the port: tell the peer, and stop being able to send or receive.
    /// Idempotent.
    pub fn close(&mut self) {
        if let Some(outbox) = self.outbox.take() {
            // The peer may already be gone; that is exactly the case this
            // notification exists to handle, so its failure is not one.
            let _ = outbox.send(Envelope::Close);
        }
        self.inbox = None;
    }
}

impl Drop for PortHandle {
    fn drop(&mut self) {
        self.close();
    }
}
