//! Event-driven XFixes monitoring with bounded ICCCM incremental transfers.
use crate::{
    clipboard::{Command, Event, RawCapture},
    content::{Representation, MAX_ITEM, MIME_ALLOWLIST},
    security::{self, Gate},
};
use std::sync::atomic::Ordering;
use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, Sender, SyncSender},
    time::{Duration, Instant},
};
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::{
    connection::Connection,
    protocol::{
        xfixes::{self, ConnectionExt as _},
        xproto::*,
        Event as XEvent,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};
struct Transfer {
    owner: u32,
    generation: u64,
    epoch: u64,
    targets: VecDeque<(Atom, String)>,
    current: Atom,
    mime: String,
    reps: Vec<Representation>,
    bytes: Vec<u8>,
    incremental: bool,
    total: usize,
    started: Instant,
    progress: Instant,
    source: String,
}
struct Backend {
    conn: RustConnection,
    window: Window,
    clipboard: Atom,
    manager: Atom,
    targets: Atom,
    property: Atom,
    incr: Atom,
    save: Atom,
    atom: Atom,
    generation: u64,
    owner: Window,
    local: Window,
    is_manager: bool,
    transfer: Option<Transfer>,
    approved: Option<(u32, u64, u64, Vec<Representation>)>,
    requests: Vec<SelectionRequestEvent>,
    gate: Gate,
    raw: SyncSender<RawCapture>,
    events: Sender<Event>,
}
/// Start an X11 backend; clipboard notifications never run on GTK's main thread.
pub fn start(gate: Gate, raw: SyncSender<RawCapture>, events: Sender<Event>) -> Sender<Command> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Err(e) = run(gate, raw, events.clone(), rx) {
            let _ = events.send(Event::Error(e));
        }
    });
    tx
}
fn run(
    gate: Gate,
    raw: SyncSender<RawCapture>,
    events: Sender<Event>,
    rx: Receiver<Command>,
) -> Result<(), &'static str> {
    let (conn, screen) = x11rb::connect(None).map_err(|_| "x11-connect")?;
    let root = conn.setup().roots[screen].root;
    conn.xfixes_query_version(5, 0)
        .map_err(|_| "xfixes-unavailable")?
        .reply()
        .map_err(|_| "xfixes-unavailable")?;
    let atom = |name: &str| -> Result<Atom, &'static str> {
        Ok(conn
            .intern_atom(false, name.as_bytes())
            .map_err(|_| "x11-atom")?
            .reply()
            .map_err(|_| "x11-atom")?
            .atom)
    };
    let (clipboard, manager, targets, property, incr, save) = (
        atom("CLIPBOARD")?,
        atom("CLIPBOARD_MANAGER")?,
        atom("TARGETS")?,
        atom("_CLIPLEDGE_TRANSFER")?,
        atom("INCR")?,
        atom("SAVE_TARGETS")?,
    );
    let window = conn.generate_id().map_err(|_| "x11-window")?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(|_| "x11-window")?;
    let is_manager = conn
        .get_selection_owner(manager)
        .map_err(|_| "x11-manager")?
        .reply()
        .map_err(|_| "x11-manager")?
        .owner
        == 0;
    if is_manager {
        conn.set_selection_owner(window, manager, x11rb::CURRENT_TIME)
            .map_err(|_| "x11-manager")?;
    }
    conn.xfixes_select_selection_input(
        window,
        clipboard,
        xfixes::SelectionEventMask::SET_SELECTION_OWNER
            | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
            | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )
    .map_err(|_| "xfixes-monitor")?;
    conn.flush().map_err(|_| "x11-flush")?;
    let _ = events.send(Event::Manager(is_manager));
    let mut b = Backend {
        conn,
        window,
        clipboard,
        manager,
        targets,
        property,
        incr,
        save,
        atom: AtomEnum::ATOM.into(),
        generation: 0,
        owner: 0,
        local: 0,
        is_manager,
        transfer: None,
        approved: None,
        requests: Vec::new(),
        gate,
        raw,
        events,
    };
    loop {
        while let Ok(c) = rx.try_recv() {
            match c {
                Command::Stop => return Ok(()),
                Command::LocalOwner => {
                    b.local = b
                        .conn
                        .get_selection_owner(b.clipboard)
                        .map_err(|_| "x11-owner")?
                        .reply()
                        .map_err(|_| "x11-owner")?
                        .owner;
                    b.transfer = None;
                    b.approved = None;
                }
                Command::Published { request } => {
                    if let Some(r) = request {
                        b.notify(r, true);
                    }
                }
                Command::Approved {
                    owner,
                    generation,
                    epoch,
                    reps,
                } => {
                    if b.owner == owner && b.generation == generation && b.gate.accepts(epoch) {
                        b.approved = Some((owner, generation, epoch, reps));
                        b.serve_requests();
                    }
                }
            }
        }
        if !b.gate.eligible() {
            b.transfer = None;
            b.approved = None;
            let pending = std::mem::take(&mut b.requests);
            for r in pending {
                b.notify(r, false);
            }
        }
        if b.transfer
            .as_ref()
            .map(|t| {
                t.started.elapsed() > Duration::from_secs(10)
                    || t.progress.elapsed() > Duration::from_secs(2)
            })
            .unwrap_or(false)
        {
            b.transfer = None;
            let pending = std::mem::take(&mut b.requests);
            for r in pending {
                b.notify(r, false);
            }
        }
        while let Some(event) = b.conn.poll_for_event().map_err(|_| "x11-disconnected")? {
            b.event(event);
        }
        let _ = b.conn.flush();
        std::thread::sleep(Duration::from_millis(5));
    }
}
impl Backend {
    fn event(&mut self, event: XEvent) {
        match event {
            XEvent::XfixesSelectionNotify(e) => {
                let old_owner = self.owner;
                // Selection destruction is different from an explicit SetSelectionOwner(None).
                if e.owner == 0
                    && matches!(
                        e.subtype,
                        xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY
                            | xfixes::SelectionEvent::SELECTION_CLIENT_CLOSE
                    )
                {
                    if self.is_manager {
                        if let Some((owner, generation, epoch, reps)) = self.approved.take() {
                            if owner == old_owner && self.gate.accepts(epoch) {
                                let _ = self.events.send(Event::Publish {
                                    reps,
                                    epoch,
                                    generation: generation.wrapping_add(1),
                                    request: None,
                                });
                            }
                        }
                    }
                }
                self.generation = self.generation.wrapping_add(1);
                self.gate.ownership.store(self.generation, Ordering::SeqCst);
                self.owner = e.owner;
                self.transfer = None;
                self.approved = None;
                if e.owner != 0 && e.owner != self.local && self.gate.eligible() {
                    self.begin(e.owner, e.timestamp);
                }
            }
            XEvent::SelectionNotify(e) => {
                if e.requestor == self.window && e.selection == self.clipboard {
                    if e.property == 0 {
                        self.next();
                    } else {
                        self.read_property(false);
                    }
                }
            }
            XEvent::PropertyNotify(e) => {
                if e.window == self.window
                    && e.atom == self.property
                    && e.state == Property::NEW_VALUE
                    && self
                        .transfer
                        .as_ref()
                        .map(|t| t.incremental)
                        .unwrap_or(false)
                {
                    self.read_property(true);
                }
            }
            XEvent::SelectionRequest(r) => {
                if r.selection != self.manager || !self.is_manager {
                    return;
                }
                if r.target == self.targets {
                    let prop = if r.property == 0 {
                        r.target
                    } else {
                        r.property
                    };
                    let _ = self.conn.change_property32(
                        PropMode::REPLACE,
                        r.requestor,
                        prop,
                        self.atom,
                        &[self.targets, self.save],
                    );
                    self.notify(r, true);
                } else if r.target == self.save
                    && self.gate.eligible()
                    && r.requestor == self.owner
                    && self.requests.is_empty()
                {
                    self.requests.push(r);
                    self.serve_requests();
                } else {
                    self.notify(r, false);
                }
            }
            XEvent::SelectionClear(e) => {
                if e.selection == self.manager {
                    self.is_manager = false;
                    self.approved = None;
                    let _ = self.events.send(Event::Manager(false));
                }
            }
            _ => {}
        }
    }
    fn begin(&mut self, owner: u32, time: u32) {
        let source = self.source(owner);
        if self.is_self(owner) {
            self.local = owner;
            return;
        }
        self.transfer = Some(Transfer {
            owner,
            generation: self.generation,
            epoch: self.gate.generation(),
            targets: VecDeque::new(),
            current: self.targets,
            mime: String::new(),
            reps: vec![],
            bytes: vec![],
            incremental: false,
            total: 0,
            started: Instant::now(),
            progress: Instant::now(),
            source,
        });
        let _ = self.conn.delete_property(self.window, self.property);
        let _ = self.conn.convert_selection(
            self.window,
            self.clipboard,
            self.targets,
            self.property,
            time,
        );
    }
    fn source(&self, owner: u32) -> String {
        let atom = |name: &str| {
            self.conn
                .intern_atom(false, name.as_bytes())
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.atom)
                .unwrap_or(0)
        };
        let cardinal = |window: u32, property: Atom| {
            self.conn
                .get_property(false, window, property, AtomEnum::ANY, 0, 1)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32().and_then(|mut v| v.next()))
        };
        let leader = cardinal(owner, atom("WM_CLIENT_LEADER")).unwrap_or(owner);
        for window in [owner, leader] {
            let class = self
                .conn
                .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|p| {
                    String::from_utf8_lossy(&p.value)
                        .split('\0')
                        .filter(|s| !s.is_empty())
                        .last()
                        .unwrap_or("")
                        .to_lowercase()
                })
                .unwrap_or_default();
            if !class.is_empty() {
                return class;
            }
        }
        for window in [owner, leader] {
            if let Some(pid) = cardinal(window, atom("_NET_WM_PID")) {
                if let Ok(name) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
                    return name
                        .trim()
                        .chars()
                        .take(128)
                        .collect::<String>()
                        .to_lowercase();
                }
            }
        }
        String::new()
    }
    fn is_self(&self, owner: u32) -> bool {
        let a = self
            .conn
            .intern_atom(false, b"_NET_WM_PID")
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom)
            .unwrap_or(0);
        self.conn
            .get_property(false, owner, a, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()))
            .map(|p| p == std::process::id())
            .unwrap_or(false)
    }
    fn read_property(&mut self, chunk: bool) {
        let Some(t) = self.transfer.as_ref() else {
            return;
        };
        if !self.gate.accepts(t.epoch) {
            self.transfer = None;
            return;
        }
        let left = if t.current == self.targets {
            64 * 1024
        } else {
            MAX_ITEM.saturating_sub(t.total + t.bytes.len())
        };
        let reply = self
            .conn
            .get_property(
                true,
                self.window,
                self.property,
                AtomEnum::ANY,
                0,
                (left / 4 + 1) as u32,
            )
            .ok()
            .and_then(|c| c.reply().ok());
        let Some(p) = reply else {
            self.transfer = None;
            return;
        };
        if p.bytes_after > 0 || p.value.len() > left {
            self.transfer = None;
            return;
        }
        let t = self.transfer.as_mut().unwrap();
        t.progress = Instant::now();
        if p.type_ == self.incr && !chunk {
            t.incremental = true;
            return;
        }
        if t.current == self.targets {
            if p.format != 32 {
                self.transfer = None;
                return;
            }
            let atoms = p
                .value32()
                .map(|v| v.collect::<Vec<_>>())
                .unwrap_or_default();
            let mut offered = Vec::new();
            if atoms.len() > 256 {
                self.transfer = None;
                return;
            }
            for a in atoms {
                if let Some(name) = self.conn.get_atom_name(a).ok().and_then(|c| c.reply().ok()) {
                    let name = String::from_utf8_lossy(&name.name).into_owned();
                    if security::sensitive_hint(&name) {
                        self.transfer = None;
                        return;
                    }
                    if MIME_ALLOWLIST.contains(&name.as_str()) {
                        offered.push((a, name));
                    }
                }
            }
            if let Some(t) = self.transfer.as_mut() {
                t.targets = offered.into();
            }
            self.next();
            return;
        }
        if p.format != 8 && !p.value.is_empty() {
            self.transfer = None;
            return;
        }
        if chunk && !p.value.is_empty() {
            t.bytes.extend_from_slice(&p.value);
            return;
        }
        if !chunk {
            t.bytes = p.value;
        }
        let bytes = std::mem::take(&mut t.bytes);
        t.total += bytes.len();
        if !bytes.is_empty() {
            t.reps.push(Representation::new(&t.mime, bytes));
        }
        t.incremental = false;
        self.next();
    }
    fn next(&mut self) {
        let Some(t) = self.transfer.as_mut() else {
            return;
        };
        if let Some((a, name)) = t.targets.pop_front() {
            t.current = a;
            t.mime = name;
            t.bytes.clear();
            t.incremental = false;
            let _ = self.conn.delete_property(self.window, self.property);
            let _ = self.conn.convert_selection(
                self.window,
                self.clipboard,
                a,
                self.property,
                x11rb::CURRENT_TIME,
            );
        } else {
            let t = self.transfer.take().unwrap();
            if !t.reps.is_empty() && self.gate.accepts(t.epoch) {
                if self
                    .raw
                    .try_send(RawCapture {
                        owner: t.owner,
                        generation: t.generation,
                        epoch: t.epoch,
                        reps: t.reps,
                        source: t.source,
                    })
                    .is_err()
                {
                    self.gate.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    fn serve_requests(&mut self) {
        if let Some((_, generation, epoch, reps)) = &self.approved {
            if self.gate.accepts(*epoch) {
                for request in std::mem::take(&mut self.requests) {
                    let _ = self.events.send(Event::Publish {
                        reps: reps.clone(),
                        epoch: *epoch,
                        generation: *generation,
                        request: Some(request),
                    });
                }
            }
        }
    }
    fn notify(&self, r: SelectionRequestEvent, success: bool) {
        let property = if success {
            if r.property == 0 {
                r.target
            } else {
                r.property
            }
        } else {
            0
        };
        let ev = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: r.time,
            requestor: r.requestor,
            selection: r.selection,
            target: r.target,
            property,
        };
        let _ = self
            .conn
            .send_event(false, r.requestor, EventMask::NO_EVENT, ev);
    }
}
