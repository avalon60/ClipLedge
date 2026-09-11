//! Bounded CPU capture, independent encrypted reads, and serialized database writes.
use crate::{
    clipboard::{self, RawCapture},
    content::{self, Capture, Representation},
    security::Gate,
    settings::Settings,
    storage::{Item, Storage},
};
use std::sync::{
    atomic::Ordering,
    mpsc::{self, Receiver, Sender},
    Arc, RwLock,
};
use zeroize::Zeroizing;
/// Requests from GTK. No GTK objects cross into workers.
pub enum Request {
    Unlock(bool),
    Search {
        query: String,
        filter: String,
        serial: u64,
        offset: u32,
    },
    Restore(i64),
    Touch(i64),
    Pin(i64),
    Delete(i64, bool),
    Clear(bool),
    Settings(Settings),
    Lock,
    Reset,
    Stop,
}
/// Bounded results delivered to the GTK thread.
pub enum Response {
    Status {
        state: &'static str,
        count: u64,
        version: String,
    },
    Items {
        serial: u64,
        items: Vec<Item>,
        total: u64,
    },
    Restore {
        reps: Vec<Representation>,
        epoch: u64,
    },
    Changed,
    Error(&'static str),
}
enum ReadRequest {
    Open(Zeroizing<Vec<u8>>, Sender<Result<(), &'static str>>),
    Close(Sender<()>),
    Search {
        query: String,
        filter: String,
        serial: u64,
        offset: u32,
    },
    Restore(i64, u64),
    Stop,
}
struct Prepared {
    owner: u32,
    generation: u64,
    epoch: u64,
    content: Capture,
}
/// UI handles for long-lived background workers.
pub struct Engine {
    pub tx: Sender<Request>,
    pub rx: Receiver<Response>,
    pub clip_rx: Receiver<clipboard::Event>,
    pub clip_tx: Option<Sender<clipboard::Command>>,
    pub gate: Gate,
    reader: Sender<ReadRequest>,
}
impl Engine {
    /// Start X11 capture only on a verified native X11 session.
    pub fn start(x11: bool) -> Self {
        let gate = Gate::default();
        gate.backend.store(x11, Ordering::SeqCst);
        gate.capacity.store(true, Ordering::SeqCst);
        let (tx, requests) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        // At most 25 MiB in either queue; one decoder owns the in-flight item.
        let (raw_tx, raw) = mpsc::sync_channel::<RawCapture>(1);
        let (prepared_tx, prepared) = mpsc::sync_channel(1);
        let (clip_out, clip_rx) = mpsc::channel();
        let clip_tx = if x11 {
            Some(clipboard::x11::start(gate.clone(), raw_tx, clip_out))
        } else {
            None
        };
        let settings = Arc::new(RwLock::new(Settings::load()));
        let cfg = settings.clone();
        let g = gate.clone();
        std::thread::spawn(move || {
            while let Ok(job) = raw.recv() {
                if !g.accepts(job.epoch) {
                    continue;
                }
                let settings = cfg.read().unwrap().clone();
                if let Ok(content) = content::prepare(job.reps, job.source, &settings) {
                    if g.accepts(job.epoch) {
                        let _ = prepared_tx.try_send(Prepared {
                            owner: job.owner,
                            generation: job.generation,
                            epoch: job.epoch,
                            content,
                        });
                    }
                }
            }
        });
        let (reader, reads) = mpsc::channel();
        let o = out.clone();
        let g = gate.clone();
        std::thread::spawn(move || reader_worker(reads, o, g));
        let g = gate.clone();
        let x = clip_tx.clone();
        let read_handle = reader.clone();
        std::thread::spawn(move || writer_worker(requests, out, prepared, x, g, settings, reader));
        Self {
            tx,
            rx,
            clip_rx,
            clip_tx,
            gate,
            reader: read_handle,
        }
    }
    /// Search independently of the writer's maintenance queue.
    pub fn search(&self, query: String, filter: String, serial: u64, offset: u32) {
        let _ = self.reader.send(ReadRequest::Search {
            query,
            filter,
            serial,
            offset,
        });
    }
    /// Restore through the independent reader, even while writer maintenance runs.
    pub fn restore(&self, id: i64) {
        if self.gate.key_ready.load(Ordering::SeqCst) && !self.gate.locked.load(Ordering::SeqCst) {
            let _ = self
                .reader
                .send(ReadRequest::Restore(id, self.gate.generation()));
            let _ = self.tx.send(Request::Touch(id));
        }
    }
}
fn reader_worker(requests: Receiver<ReadRequest>, out: Sender<Response>, gate: Gate) {
    let mut db: Option<Storage> = None;
    while let Ok(req) = requests.recv() {
        match req {
            ReadRequest::Stop => return,
            ReadRequest::Close(ack) => {
                db = None;
                let _ = ack.send(());
            }
            ReadRequest::Open(key, ack) => {
                let result =
                    Storage::open_reader(&crate::settings::data_dir().join("history.db"), &key)
                        .map(|s| {
                            db = Some(s);
                        });
                let _ = ack.send(result);
            }
            ReadRequest::Search {
                query,
                filter,
                serial,
                offset,
            } => {
                if !gate.key_ready.load(Ordering::SeqCst) || gate.locked.load(Ordering::SeqCst) {
                    let _ = out.send(Response::Items {
                        serial,
                        items: vec![],
                        total: 0,
                    });
                    continue;
                }
                if let Some(d) = &db {
                    match d.search_page(&query, &filter, offset) {
                        Ok(items) => {
                            let _ = out.send(Response::Items {
                                serial,
                                items,
                                total: d.count().unwrap_or(0),
                            });
                        }
                        Err(e) => {
                            let _ = out.send(Response::Error(e));
                        }
                    }
                }
            }
            ReadRequest::Restore(id, epoch) => {
                if gate.key_ready.load(Ordering::SeqCst)
                    && gate.generation() == epoch
                    && !gate.locked.load(Ordering::SeqCst)
                {
                    if let Some(d) = &db {
                        match d.representations(id) {
                            Ok(reps) => {
                                let _ = out.send(Response::Restore { reps, epoch });
                            }
                            Err(e) => {
                                let _ = out.send(Response::Error(e));
                            }
                        }
                    }
                }
            }
        }
    }
}
fn close_reader(reader: &Sender<ReadRequest>) {
    let (tx, rx) = mpsc::channel();
    if reader.send(ReadRequest::Close(tx)).is_ok() {
        let _ = rx.recv();
    }
}
fn writer_worker(
    requests: Receiver<Request>,
    out: Sender<Response>,
    prepared: Receiver<Prepared>,
    clip: Option<Sender<clipboard::Command>>,
    gate: Gate,
    settings: Arc<RwLock<Settings>>,
    reader: Sender<ReadRequest>,
) {
    let mut db: Option<Storage> = None;
    loop {
        // Interactive commands are always drained before another capture commit.
        while let Ok(request) = requests.try_recv() {
            let result: Result<(), &'static str> = (|| {
                match request {
                    Request::Stop => {
                        gate.key_ready.store(false, Ordering::SeqCst);
                        let _ = reader.send(ReadRequest::Stop);
                        if let Some(x) = &clip {
                            let _ = x.send(clipboard::Command::Stop);
                        }
                        return Err("stop");
                    }
                    Request::Lock | Request::Reset => {
                        let reset = matches!(request, Request::Reset);
                        gate.key_ready.store(false, Ordering::SeqCst);
                        gate.invalidate();
                        db = None;
                        close_reader(&reader);
                        if reset {
                            crate::keyring::delete_key()?;
                            for name in [
                                "history.db",
                                "history.db-wal",
                                "history.db-shm",
                                "history.recovery.db",
                            ] {
                                let p = crate::settings::data_dir().join(name);
                                if p.exists() {
                                    std::fs::remove_file(p).map_err(|_| "reset-files")?;
                                }
                            }
                        }
                        let _ = out.send(Response::Status {
                            state: if reset {
                                "reset-unlock-to-start"
                            } else {
                                "locked"
                            },
                            count: 0,
                            version: String::new(),
                        });
                        let _ = out.send(Response::Changed);
                    }
                    Request::Unlock(interactive) => {
                        if gate.locked.load(Ordering::SeqCst) {
                            return Err("session-locked");
                        }
                        let epoch = gate.generation();
                        let path = crate::settings::data_dir().join("history.db");
                        let key = crate::keyring::database_key(interactive, path.exists())?;
                        if gate.locked.load(Ordering::SeqCst) || gate.generation() != epoch {
                            return Err("unlock-cancelled");
                        }
                        let mut opened = Storage::open(&path, &key)?;
                        opened.expire(&settings.read().unwrap())?;
                        let count = opened.count()?;
                        let version = opened.cipher_version.clone();
                        let (tx, rx) = mpsc::channel();
                        reader
                            .send(ReadRequest::Open(key, tx))
                            .map_err(|_| "reader-stopped")?;
                        rx.recv().map_err(|_| "reader-stopped")??;
                        if gate.locked.load(Ordering::SeqCst) || gate.generation() != epoch {
                            close_reader(&reader);
                            return Err("unlock-cancelled");
                        }
                        db = Some(opened);
                        gate.key_ready.store(true, Ordering::SeqCst);
                        gate.capacity.store(true, Ordering::SeqCst);
                        let _ = out.send(Response::Status {
                            state: "ready",
                            count,
                            version,
                        });
                        let _ = out.send(Response::Changed);
                    }
                    Request::Settings(s) => {
                        *settings.write().unwrap() = s;
                        gate.invalidate();
                        gate.capacity.store(true, Ordering::SeqCst);
                        if let Some(d) = &mut db {
                            d.expire(&settings.read().unwrap())?;
                        }
                        let _ = out.send(Response::Changed);
                    }
                    Request::Search {
                        query,
                        filter,
                        serial,
                        offset,
                    } => {
                        reader
                            .send(ReadRequest::Search {
                                query,
                                filter,
                                serial,
                                offset,
                            })
                            .map_err(|_| "reader-stopped")?;
                    }
                    Request::Touch(id) => {
                        if let Some(d) = &db {
                            d.touch(id)?;
                        }
                    }
                    Request::Restore(id) => {
                        if gate.locked.load(Ordering::SeqCst)
                            || !gate.key_ready.load(Ordering::SeqCst)
                        {
                            return Err("history-locked");
                        }
                        db.as_ref().ok_or("history-locked")?.touch(id)?;
                        let _ = reader.send(ReadRequest::Restore(id, gate.generation()));
                    }
                    Request::Pin(id) => {
                        db.as_ref().ok_or("history-locked")?.pin(id)?;
                        let _ = out.send(Response::Changed);
                    }
                    Request::Delete(id, confirmed) => {
                        db.as_mut().ok_or("history-locked")?.delete(id, confirmed)?;
                        gate.capacity.store(true, Ordering::SeqCst);
                        let _ = out.send(Response::Changed);
                    }
                    Request::Clear(pins) => {
                        gate.invalidate();
                        db.as_mut().ok_or("history-locked")?.clear(pins)?;
                        gate.capacity.store(true, Ordering::SeqCst);
                        let _ = out.send(Response::Changed);
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                if e == "stop" {
                    return;
                }
                if e.starts_with("storage-") {
                    gate.capacity.store(false, Ordering::SeqCst);
                    gate.invalidate();
                }
                let _ = out.send(Response::Error(e));
            }
        }
        if let Ok(job) = prepared.try_recv() {
            if gate.accepts(job.epoch) {
                if let Some(d) = &mut db {
                    let cfg = settings.read().unwrap().clone();
                    match d.insert_if(&job.content, &cfg, || gate.accepts(job.epoch)) {
                        Ok(_) => {
                            if gate.accepts(job.epoch) {
                                if let Some(x) = &clip {
                                    let _ = x.send(clipboard::Command::Approved {
                                        owner: job.owner,
                                        generation: job.generation,
                                        epoch: job.epoch,
                                        reps: job.content.reps,
                                    });
                                }
                                let _ = out.send(Response::Changed);
                            }
                        }
                        Err("capture-cancelled") => {}
                        Err(e) => {
                            gate.capacity.store(false, Ordering::SeqCst);
                            gate.invalidate();
                            let _ = out.send(Response::Error(e));
                        }
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
