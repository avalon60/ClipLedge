//! Isolated X11 protocol tests using independent source and manager connections.
// Author: Clive Bostock
// Date: 10-Sep-2026
// Purpose: Validate transfer, privacy, and clipboard-preservation protocol edges.
// Usage: xvfb-run -a cargo test --test x11_backend -- --ignored --test-threads=1
use clipledge::{
    clipboard::{self, Command, Event},
    security::Gate,
};
use std::{
    sync::{atomic::Ordering, mpsc},
    thread,
    time::Duration,
};
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event as XEvent},
    wrapper::ConnectionExt as _,
};
enum SourceAction {
    Exit,
    Clear,
    Save,
}
struct Source {
    stop: mpsc::Sender<SourceAction>,
    saved: mpsc::Receiver<bool>,
    thread: Option<thread::JoinHandle<()>>,
    pub owner: u32,
}
impl Drop for Source {
    fn drop(&mut self) {
        let _ = self.stop.send(SourceAction::Exit);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}
fn source(secret: bool, large: bool) -> Source {
    let (tx, rx) = mpsc::channel();
    let (ready, wait) = mpsc::channel();
    let (saved, saved_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (c, s) = x11rb::connect(None).unwrap();
        let root = c.setup().roots[s].root;
        let w = c.generate_id().unwrap();
        c.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            w,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new(),
        )
        .unwrap();
        let atom = |s: &str| {
            c.intern_atom(false, s.as_bytes())
                .unwrap()
                .reply()
                .unwrap()
                .atom
        };
        let clip = atom("CLIPBOARD");
        let targets = atom("TARGETS");
        let utf8 = atom("UTF8_STRING");
        let html = atom("text/html");
        let hint = atom("x-kde-passwordManagerHint");
        let incr = atom("INCR");
        c.set_selection_owner(w, clip, x11rb::CURRENT_TIME).unwrap();
        c.flush().unwrap();
        ready.send(w).unwrap();
        let mut transfer: Option<(u32, u32, usize)> = None;
        loop {
            if let Ok(action) = rx.try_recv() {
                match action {
                    SourceAction::Exit => break,
                    SourceAction::Clear => {
                        c.set_selection_owner(0u32, clip, x11rb::CURRENT_TIME)
                            .unwrap();
                        c.flush().unwrap();
                        thread::sleep(Duration::from_millis(100));
                        break;
                    }
                    SourceAction::Save => {
                        c.convert_selection(
                            w,
                            atom("CLIPBOARD_MANAGER"),
                            atom("SAVE_TARGETS"),
                            atom("_TEST_SAVE"),
                            x11rb::CURRENT_TIME,
                        )
                        .unwrap();
                        c.flush().unwrap();
                    }
                }
            }
            while let Some(e) = c.poll_for_event().unwrap() {
                match e {
                    XEvent::SelectionRequest(r) => {
                        let property = if r.property == 0 {
                            r.target
                        } else {
                            r.property
                        };
                        if r.target == targets {
                            let mut a = vec![targets, utf8, html];
                            if secret {
                                a.push(hint);
                            }
                            c.change_property32(
                                PropMode::REPLACE,
                                r.requestor,
                                property,
                                AtomEnum::ATOM,
                                &a,
                            )
                            .unwrap();
                        } else if r.target == utf8 && large {
                            c.change_window_attributes(
                                r.requestor,
                                &ChangeWindowAttributesAux::new()
                                    .event_mask(EventMask::PROPERTY_CHANGE),
                            )
                            .unwrap();
                            c.change_property32(
                                PropMode::REPLACE,
                                r.requestor,
                                property,
                                incr,
                                &[196608],
                            )
                            .unwrap();
                            transfer = Some((r.requestor, property, 0));
                        } else {
                            let bytes = if r.target == html {
                                b"<b>clipboard fixture</b>".as_slice()
                            } else {
                                b"clipboard fixture".as_slice()
                            };
                            c.change_property8(
                                PropMode::REPLACE,
                                r.requestor,
                                property,
                                r.target,
                                bytes,
                            )
                            .unwrap();
                        }
                        c.send_event(
                            false,
                            r.requestor,
                            EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: r.time,
                                requestor: r.requestor,
                                selection: r.selection,
                                target: r.target,
                                property,
                            },
                        )
                        .unwrap();
                    }
                    XEvent::SelectionNotify(n) => {
                        let _ = saved.send(n.property != 0);
                    }
                    XEvent::PropertyNotify(p) => {
                        if p.state == Property::DELETE {
                            if let Some((w, a, n)) = transfer {
                                if p.window == w && p.atom == a {
                                    let bytes = if n < 3 { vec![b'x'; 65536] } else { vec![] };
                                    c.change_property8(PropMode::REPLACE, w, a, utf8, &bytes)
                                        .unwrap();
                                    transfer = if n < 3 { Some((w, a, n + 1)) } else { None };
                                }
                            }
                        }
                    }
                    _ => {}
                }
                c.flush().unwrap();
            }
            thread::sleep(Duration::from_millis(2));
        }
    });
    Source {
        stop: tx,
        saved: saved_rx,
        thread: Some(handle),
        owner: wait.recv_timeout(Duration::from_secs(3)).unwrap(),
    }
}
fn gate() -> Gate {
    let g = Gate::default();
    g.key_ready.store(true, Ordering::SeqCst);
    g.backend.store(true, Ordering::SeqCst);
    g.capacity.store(true, Ordering::SeqCst);
    g
}
#[test]
#[ignore = "requires an isolated Xvfb display"]
fn capture_incremental_sensitive_hints_and_preservation() {
    let gate = gate();
    let (raw_tx, raw_rx) = mpsc::sync_channel(1);
    let (ev_tx, ev_rx) = mpsc::channel();
    let control = clipboard::x11::start(gate.clone(), raw_tx, ev_tx);
    assert!(matches!(
        ev_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        Event::Manager(true)
    ));
    let src = source(false, true);
    let raw = raw_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(raw.owner, src.owner);
    assert_eq!(
        raw.reps
            .iter()
            .find(|r| r.mime == "UTF8_STRING")
            .unwrap()
            .bytes
            .len(),
        196608
    );
    assert!(raw.reps.iter().any(|r| r.mime == "text/html"));
    control
        .send(Command::Approved {
            owner: raw.owner,
            generation: raw.generation,
            epoch: raw.epoch,
            reps: raw.reps,
        })
        .unwrap();
    thread::sleep(Duration::from_millis(50));
    src.stop.send(SourceAction::Save).unwrap();
    if let Event::Publish {
        request: Some(request),
        ..
    } = ev_rx.recv_timeout(Duration::from_secs(3)).unwrap()
    {
        control
            .send(Command::Published {
                request: Some(request),
            })
            .unwrap();
    } else {
        panic!("SAVE_TARGETS not coordinated");
    }
    assert!(src.saved.recv_timeout(Duration::from_secs(3)).unwrap());
    drop(src);
    match ev_rx.recv_timeout(Duration::from_secs(3)).unwrap() {
        Event::Publish {
            generation, epoch, ..
        } => {
            assert!(gate.accepts(epoch));
            thread::sleep(Duration::from_millis(20));
            assert_eq!(gate.ownership.load(Ordering::SeqCst), generation);
        }
        _ => panic!("expected preservation"),
    }
    let src = source(true, false);
    assert!(raw_rx.recv_timeout(Duration::from_millis(400)).is_err());
    drop(src);
    assert!(ev_rx.recv_timeout(Duration::from_millis(100)).is_err());
    let mut src = source(false, false);
    let raw = raw_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    control
        .send(Command::Approved {
            owner: raw.owner,
            generation: raw.generation,
            epoch: raw.epoch,
            reps: raw.reps,
        })
        .unwrap();
    thread::sleep(Duration::from_millis(30));
    src.stop.send(SourceAction::Clear).unwrap();
    src.thread.take().unwrap().join().unwrap();
    assert!(ev_rx.recv_timeout(Duration::from_millis(200)).is_err());
    gate.paused.store(true, Ordering::SeqCst);
    gate.invalidate();
    let src = source(false, false);
    assert!(raw_rx.recv_timeout(Duration::from_millis(200)).is_err());
    drop(src);
    control.send(Command::Stop).unwrap();
}
#[test]
#[ignore = "requires an isolated Xvfb display"]
fn existing_manager_is_not_stolen() {
    let (c, s) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[s].root;
    let w = c.generate_id().unwrap();
    c.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        w,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .unwrap();
    let manager = c
        .intern_atom(false, b"CLIPBOARD_MANAGER")
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    c.set_selection_owner(w, manager, x11rb::CURRENT_TIME)
        .unwrap();
    c.flush().unwrap();
    let (raw, _) = mpsc::sync_channel(1);
    let (ev, rx) = mpsc::channel();
    let control = clipboard::x11::start(gate(), raw, ev);
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Event::Manager(false)
    ));
    assert_eq!(
        c.get_selection_owner(manager)
            .unwrap()
            .reply()
            .unwrap()
            .owner,
        w
    );
    control.send(Command::Stop).unwrap();
    thread::sleep(Duration::from_millis(20));
}
