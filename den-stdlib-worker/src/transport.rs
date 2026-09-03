//! The channel a `MessagePort` pair — and therefore a `Worker` — is made of.
//!
//! Nothing here touches a `JSValue`: a [`PortHandle`] is plain `Send` Rust data
//! precisely so it can be moved onto a worker thread, or into a [`Message`]
//! when a port is transferred.

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender, error::SendError};

use crate::message::Message;

/// One end of an entangled pair.
///
/// Owns the sender into the *peer's* inbox and, until a pump takes it, its own
/// inbox. Dropping a handle drops that sender, which is how the peer's pump
/// learns the port is gone — its `recv()` yields `None` — instead of parking
/// forever.
#[derive(Debug)]
pub struct PortHandle {
    outbox: UnboundedSender<Message>,
    inbox:  Option<UnboundedReceiver<Message>>,
}

impl PortHandle {
    /// Two handles, entangled: what one sends, the other receives.
    pub fn pair() -> (Self, Self) {
        let (to_first, first_inbox) = mpsc::unbounded_channel();
        let (to_second, second_inbox) = mpsc::unbounded_channel();
        (
            Self {
                outbox: to_second,
                inbox:  Some(first_inbox),
            },
            Self {
                outbox: to_first,
                inbox:  Some(second_inbox),
            },
        )
    }

    /// Hand `message` to the peer. The message comes back in the `Err` when
    /// there is no peer left to take it — which is not an error in the spec:
    /// posting to a closed port is silently a no-op.
    pub fn send(&self, message: Message) -> Result<(), SendError<Message>> {
        self.outbox.send(message)
    }

    /// Take this end's inbox, once, for a pump to await. `None` afterwards.
    pub const fn take_receiver(&mut self) -> Option<UnboundedReceiver<Message>> {
        self.inbox.take()
    }
}
