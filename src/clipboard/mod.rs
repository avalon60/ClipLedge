//! Clipboard backend protocol and bounded X11 transport.
pub mod x11;
use crate::content::Representation;
/// Captured bytes tagged with both privacy and X11 ownership generations.
pub struct RawCapture {
    pub owner: u32,
    pub generation: u64,
    pub epoch: u64,
    pub reps: Vec<Representation>,
    pub source: String,
}
/// Commands for the X11 connection's owning thread.
pub enum Command {
    Approved {
        owner: u32,
        generation: u64,
        epoch: u64,
        reps: Vec<Representation>,
    },
    Published {
        request: Option<x11rb::protocol::xproto::SelectionRequestEvent>,
    },
    LocalOwner,
    Stop,
}
/// Events requiring GTK publication or non-sensitive diagnostics.
pub enum Event {
    Publish {
        reps: Vec<Representation>,
        epoch: u64,
        generation: u64,
        request: Option<x11rb::protocol::xproto::SelectionRequestEvent>,
    },
    Manager(bool),
    Error(&'static str),
}
