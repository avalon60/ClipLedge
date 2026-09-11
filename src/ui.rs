//! Theme-respecting shelf, keyboard navigation, settings, and desktop actions.
use crate::{
    clipboard,
    content::Representation,
    desktop,
    engine::{Engine, Request, Response},
    settings::Settings,
    storage::Item,
};
use gtk::{gdk, gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::Ordering,
    time::Duration,
};
/// One persistent GTK shelf and its worker handles.
pub struct Shelf {
    pub window: gtk::ApplicationWindow,
    app: gtk::Application,
    engine: Engine,
    search: gtk::SearchEntry,
    flow: gtk::FlowBox,
    status: gtk::Label,
    count: gtk::Label,
    total: Cell<u64>,
    delegated: Cell<bool>,
    items: RefCell<Vec<Item>>,
    filter: RefCell<String>,
    serial: Cell<u64>,
    offset: Cell<u32>,
    initializing: Cell<bool>,
    pending_show: Cell<bool>,
    selected: Cell<Option<i64>>,
    settings: RefCell<Settings>,
    position: Cell<desktop::Position>,
    auxiliary: Cell<u32>,
    state: RefCell<String>,
    version: RefCell<String>,
    native_x11: bool,
    provider: RefCell<Option<gdk::ContentProvider>>,
    tray: RefCell<Option<crate::tray::Tray>>,
    _hold: gio::ApplicationHoldGuard,
}
fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l
}
impl Shelf {
    /// Construct the shelf without reading the system clipboard on GTK's thread.
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let display = gdk::Display::default().expect("GTK display");
        let native_x11 = desktop::native_x11(&display);
        let settings = Settings::load();
        desktop::theme(&settings.theme);
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("ClipLedge")
            .default_width(1060)
            .default_height(340)
            .decorated(false)
            .resizable(true)
            .build();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_top(16);
        root.set_margin_bottom(12);
        root.set_margin_start(18);
        root.set_margin_end(18);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let icon = gtk::Image::from_icon_name("edit-paste-symbolic");
        icon.set_pixel_size(24);
        header.append(&icon);
        let title = label("ClipLedge");
        title.add_css_class("title-3");
        header.append(&title);
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Find something you copied")
            .hexpand(true)
            .build();
        header.append(&search);
        let pause = gtk::Button::from_icon_name("media-playback-pause-symbolic");
        pause.set_tooltip_text(Some("Pause / resume capture"));
        header.append(&pause);
        let preferences = gtk::Button::from_icon_name("emblem-system-symbolic");
        preferences.set_tooltip_text(Some("Settings"));
        header.append(&preferences);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Hide shelf (Escape)"));
        header.append(&close);
        root.append(&header);
        let filters = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let mut buttons = Vec::new();
        let mut first: Option<gtk::ToggleButton> = None;
        for name in ["Everything", "Pinned", "Text", "Images", "Links", "Files"] {
            let b = gtk::ToggleButton::with_label(name);
            if let Some(f) = &first {
                b.set_group(Some(f));
            } else {
                b.set_active(true);
                first = Some(b.clone());
            }
            filters.append(&b);
            buttons.push((name, b));
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        filters.append(&spacer);
        let count = label("0 items");
        count.add_css_class("dim-label");
        filters.append(&count);
        root.append(&filters);
        let flow = gtk::FlowBox::builder()
            .orientation(gtk::Orientation::Vertical)
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .homogeneous(true)
            .row_spacing(10)
            .column_spacing(10)
            .min_children_per_line(1)
            .max_children_per_line(1)
            .valign(gtk::Align::Start)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(205)
            .child(&flow)
            .vexpand(true)
            .build();
        root.append(&scroll);
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let hint = label("Enter: restore · Ctrl+V: paste in your application");
        hint.add_css_class("dim-label");
        hint.set_hexpand(true);
        footer.append(&hint);
        let previous = gtk::Button::from_icon_name("go-previous-symbolic");
        previous.set_tooltip_text(Some("Previous 60 items"));
        footer.append(&previous);
        let next = gtk::Button::from_icon_name("go-next-symbolic");
        next.set_tooltip_text(Some("Next 60 items"));
        footer.append(&next);
        let clear = gtk::Button::with_label("Clear history…");
        footer.append(&clear);
        root.append(&footer);
        let status = label(if native_x11 {
            "History locked — use Settings → Unlock / Retry"
        } else {
            "Wayland: background capture unavailable; browse and restore while focused."
        });
        status.set_wrap(true);
        status.add_css_class("dim-label");
        root.append(&status);
        window.set_child(Some(&root));
        let css = gtk::CssProvider::new();
        css.load_from_string(".shelf-card { padding: 12px; border: 1px solid alpha(@theme_fg_color, 0.18); border-radius: 8px; background: @theme_base_color; } flowboxchild:selected .shelf-card { border-color: @theme_selected_bg_color; } flowboxchild { padding: 2px; } .shelf-card label { color: @theme_text_color; } .card-preview { font-size: 1.05em; }");
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let this = Rc::new(Self {
            window,
            app: app.clone(),
            engine: Engine::start(native_x11),
            search,
            flow,
            status,
            count,
            total: Cell::new(0),
            delegated: Cell::new(false),
            items: RefCell::new(vec![]),
            filter: RefCell::new("Everything".into()),
            serial: Cell::new(0),
            offset: Cell::new(0),
            initializing: Cell::new(true),
            pending_show: Cell::new(false),
            selected: Cell::new(None),
            settings: RefCell::new(settings),
            position: Cell::new(desktop::Position::default()),
            auxiliary: Cell::new(0),
            state: RefCell::new("locked".into()),
            version: RefCell::new(String::new()),
            native_x11,
            provider: RefCell::new(None),
            tray: RefCell::new(None),
            _hold: app.hold(),
        });
        let weak = Rc::downgrade(&this);
        this.search.connect_search_changed(move |_| {
            if let Some(s) = weak.upgrade() {
                s.offset.set(0);
                s.refresh();
            }
        });
        let weak = Rc::downgrade(&this);
        this.search.connect_activate(move |_| {
            if let Some(s) = weak.upgrade() {
                s.restore_selected();
            }
        });
        let weak = Rc::downgrade(&this);
        this.flow.connect_selected_children_changed(move |f| {
            if let Some(s) = weak.upgrade() {
                if let Some(c) = f.selected_children().first() {
                    s.selected
                        .set(s.items.borrow().get(c.index() as usize).map(|i| i.id));
                }
            }
        });
        let weak = Rc::downgrade(&this);
        this.flow.connect_child_activated(move |_, _| {
            if let Some(s) = weak.upgrade() {
                s.restore_selected();
            }
        });
        for (name, b) in &buttons {
            let name = name.to_string();
            let weak = Rc::downgrade(&this);
            b.connect_toggled(move |b| {
                if b.is_active() {
                    if let Some(s) = weak.upgrade() {
                        *s.filter.borrow_mut() = name.clone();
                        s.offset.set(0);
                        s.selected.set(None);
                        s.refresh();
                    }
                }
            });
        }
        let weak = Rc::downgrade(&this);
        pause.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.pause(!s.engine.gate.paused.load(Ordering::SeqCst));
            }
        });
        let weak = Rc::downgrade(&this);
        preferences.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.show_settings();
            }
        });
        let weak = Rc::downgrade(&this);
        close.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.window.set_visible(false);
            }
        });
        let weak = Rc::downgrade(&this);
        clear.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.confirm_clear();
            }
        });
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |w| {
            w.set_visible(false);
            if let Some(s) = weak.upgrade() {
                s.selected.set(None);
            }
            glib::Propagation::Stop
        });
        let weak = Rc::downgrade(&this);
        this.window.connect_is_active_notify(move |w| {
            if !w.is_active() {
                let weak = weak.clone();
                glib::timeout_add_local_once(Duration::from_millis(120), move || {
                    if let Some(s) = weak.upgrade() {
                        if !s.window.is_active() && s.auxiliary.get() == 0 {
                            s.window.set_visible(false);
                        }
                    }
                });
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&this);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(s) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = mods.contains(gdk::ModifierType::ALT_MASK);
            if key == gdk::Key::Escape {
                s.window.set_visible(false);
                return glib::Propagation::Stop;
            }
            if ctrl && key == gdk::Key::f {
                s.search.grab_focus();
                return glib::Propagation::Stop;
            }
            if let Some(c) = key.to_unicode() {
                if let Some(n) = c.to_digit(10) {
                    if ctrl && (1..=6).contains(&n) {
                        buttons[(n - 1) as usize].1.set_active(true);
                        return glib::Propagation::Stop;
                    }
                    if alt && (1..=9).contains(&n) {
                        if let Some(item) = s.items.borrow().get((n - 1) as usize) {
                            s.send(Request::Restore(item.id));
                        }
                        return glib::Propagation::Stop;
                    }
                }
            }
            let editing = gtk::prelude::RootExt::focus(&s.window)
                .map(|w| w == s.search.clone().upcast::<gtk::Widget>() || w.is_ancestor(&s.search))
                .unwrap_or(false);
            if !editing {
                match key {
                    gdk::Key::p | gdk::Key::P => {
                        if let Some(id) = s.selected.get() {
                            s.send(Request::Pin(id));
                        }
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::Delete => {
                        s.delete_selected();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::space => {
                        s.preview();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }
            glib::Propagation::Proceed
        });
        this.window.add_controller(keys);
        let search_keys = gtk::EventControllerKey::new();
        search_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&this);
        search_keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Tab && !mods.contains(gdk::ModifierType::SHIFT_MASK) {
                if let Some(s) = weak.upgrade() {
                    if let Some(child) = s.flow.selected_children().first() {
                        child.grab_focus();
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        this.search.add_controller(search_keys);

        let weak = Rc::downgrade(&this);
        previous.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.offset.set(s.offset.get().saturating_sub(60));
                s.selected.set(None);
                s.refresh();
            }
        });
        let weak = Rc::downgrade(&this);
        next.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                if s.items.borrow().len() == 60 {
                    s.offset.set(s.offset.get().saturating_add(60));
                    s.selected.set(None);
                    s.refresh();
                }
            }
        });
        this.actions();
        this.watch_lock();
        this.watch_keyring();
        if this.settings.borrow().status_icon {
            this.start_tray();
        }
        let weak = Rc::downgrade(&this);
        glib::timeout_add_local(Duration::from_millis(16), move || {
            if let Some(s) = weak.upgrade() {
                s.poll();
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        // Initial unlock is scheduled after the screen-lock status probes complete.
        this
    }
    fn send(&self, r: Request) {
        match r {
            Request::Restore(id) => self.engine.restore(id),
            Request::Search {
                query,
                filter,
                serial,
                offset,
            } => self.engine.search(query, filter, serial, offset),
            other => {
                let _ = self.engine.tx.send(other);
            }
        }
    }
    fn refresh(&self) {
        let n = self.serial.get() + 1;
        self.serial.set(n);
        self.send(Request::Search {
            query: self.search.text().chars().take(1024).collect(),
            filter: self.filter.borrow().clone(),
            serial: n,
            offset: self.offset.get(),
        });
    }
    /// Show the shelf and record the previous X11 focus before activation.
    pub fn show(self: &Rc<Self>) {
        if self.engine.gate.locked.load(Ordering::SeqCst) {
            if self.initializing.get() {
                self.pending_show.set(true);
            }
            return;
        }
        if self.native_x11 {
            let p = desktop::position();
            self.position.set(p);
            self.window.set_default_size(p.width, 340);
        }
        self.selected.set(None);
        self.window.present();
        if self.native_x11 {
            desktop::place(&self.window, self.position.get());
        }
        self.search.grab_focus();
        self.refresh();
        if !self.settings.borrow().onboarded {
            self.show_settings();
        }
    }
    /// Pause only user capture; other gate reasons remain in effect.
    pub fn pause(&self, value: bool) {
        self.engine.gate.paused.store(value, Ordering::SeqCst);
        self.engine.gate.invalidate();
        self.status.set_text(if value {
            "Capture paused"
        } else if self.engine.gate.eligible() {
            "Capture active · history stays on this device"
        } else {
            "Capture unavailable — check Settings / Unlock"
        });
        if let Some(t) = self.tray.borrow().as_ref() {
            t.refresh();
        }
    }
    /// Non-sensitive CLI diagnostics.
    pub fn diagnostics(&self) -> String {
        format!("running=true\nversion={}\nbackend={}\ncapture={}\nstate={}\nhistory_items={}\ndropped_captures={}\npreservation_delegated={}\nsqlcipher={}\nsearch_index=fts4\n",env!("CARGO_PKG_VERSION"),if self.native_x11{"x11"}else{"wayland-degraded"},self.engine.gate.eligible(),self.state.borrow(),self.total.get(),self.engine.gate.dropped.load(Ordering::Relaxed),self.delegated.get(),self.version.borrow())
    }
    /// Attempt a bounded clipboard handoff before ending the application.
    pub fn quit(self: &Rc<Self>) {
        self.stop();
        let cancellable = gio::Cancellable::new();
        let app = self.app.clone();
        gtk::prelude::WidgetExt::display(&self.window)
            .clipboard()
            .store_async(glib::Priority::DEFAULT, Some(&cancellable), move |_| {
                app.quit()
            });
        let app = self.app.clone();
        glib::timeout_add_local_once(Duration::from_secs(1), move || {
            cancellable.cancel();
            app.quit();
        });
    }
    /// Stop background threads without clearing the live clipboard.
    pub fn stop(&self) {
        self.engine.gate.invalidate();
        self.engine.gate.paused.store(true, Ordering::SeqCst);
        self.send(Request::Stop);
    }
    fn restore_selected(&self) {
        if let Some(id) = self.selected.get() {
            self.send(Request::Restore(id));
        }
    }
    fn publish(&self, reps: Vec<Representation>) -> bool {
        if self.engine.gate.locked.load(Ordering::SeqCst) {
            return false;
        }
        let display = gtk::prelude::WidgetExt::display(&self.window);
        let clipboard = display.clipboard();
        let mut providers = Vec::new();
        for r in reps {
            let mime = match r.mime.as_str() {
                "UTF8_STRING" => "text/plain;charset=utf-8",
                "STRING" => "text/plain;charset=iso-8859-1",
                m => m,
            };
            providers.push(gdk::ContentProvider::for_bytes(
                mime,
                &glib::Bytes::from(&r.bytes),
            ));
        }
        providers.push(gdk::ContentProvider::for_bytes(
            "application/x-clipledge-owner",
            &glib::Bytes::from_owned(std::process::id().to_le_bytes().to_vec()),
        ));
        let provider = gdk::ContentProvider::new_union(&providers);
        if clipboard.set_content(Some(&provider)).is_err() {
            self.status.set_text("Clipboard restoration failed");
            return false;
        }
        display.flush();
        *self.provider.borrow_mut() = Some(provider);
        if let Some(x) = &self.engine.clip_tx {
            let _ = x.send(clipboard::Command::LocalOwner);
        }
        true
    }
    fn poll(self: &Rc<Self>) {
        while let Ok(response) = self.engine.rx.try_recv() {
            match response {
                Response::Status {
                    state,
                    count,
                    version,
                } => {
                    if state == "ready" {
                        self.app.withdraw_notification("clipledge-status");
                    }
                    *self.state.borrow_mut() = state.into();
                    *self.version.borrow_mut() = version;
                    self.total.set(count);
                    self.count.set_text(&format!("{count} items"));
                    self.status.set_text(if state == "ready" {
                        if self.engine.gate.paused.load(Ordering::SeqCst) {
                            "Capture paused · local encrypted history"
                        } else if self.native_x11 {
                            "Capture active · local encrypted history"
                        } else {
                            "Wayland: browse and restore; background capture unavailable"
                        }
                    } else {
                        "History locked — Unlock / Retry in Settings"
                    });
                    if let Some(t) = self.tray.borrow().as_ref() {
                        t.refresh();
                    }
                }
                Response::Items {
                    serial,
                    items,
                    total,
                } => {
                    self.total.set(total);
                    if serial == self.serial.get()
                        && self.engine.gate.key_ready.load(Ordering::SeqCst)
                        && !self.engine.gate.locked.load(Ordering::SeqCst)
                    {
                        self.render(items);
                    }
                }
                Response::Changed => self.refresh(),
                Response::Error(e) => {
                    if self.state.borrow().as_str() != e
                        && (e.starts_with("keyring-")
                            || e.starts_with("key-")
                            || e.starts_with("storage-"))
                    {
                        let notification = gio::Notification::new("ClipLedge needs attention");
                        notification.set_body(Some("History is unavailable or capture is paused. Open Settings to review the status and retry."));
                        notification.set_default_action("app.settings");
                        self.app
                            .send_notification(Some("clipledge-status"), &notification);
                    }
                    *self.state.borrow_mut() = e.into();
                    self.status
                        .set_text(&format!("{e} — use Settings to retry"));
                }
                Response::Restore { reps, epoch } => {
                    if self.engine.gate.generation() == epoch
                        && self.engine.gate.key_ready.load(Ordering::SeqCst)
                        && self.publish(reps)
                    {
                        if self.native_x11 {
                            desktop::return_focus(&self.window, self.position.get());
                        }
                        self.window.set_visible(false);
                    }
                }
            }
        }
        while let Ok(event) = self.engine.clip_rx.try_recv() {
            match event {
                clipboard::Event::Publish {
                    reps,
                    epoch,
                    generation,
                    request,
                } => {
                    if self.engine.gate.accepts(epoch)
                        && self.engine.gate.ownership.load(Ordering::SeqCst) == generation
                        && self.publish(reps)
                    {
                        if let Some(x) = &self.engine.clip_tx {
                            let _ = x.send(clipboard::Command::Published { request });
                        }
                    }
                }
                clipboard::Event::Manager(owned) => {
                    self.delegated.set(!owned);
                    if !owned {
                        self.status.set_text(
                            "Another clipboard manager owns preservation; observing history only",
                        );
                    }
                }
                clipboard::Event::Error(e) => {
                    self.engine.gate.backend.store(false, Ordering::SeqCst);
                    self.status.set_text(e);
                }
            }
        }
    }
    fn render(self: &Rc<Self>, items: Vec<Item>) {
        let selected = self.selected.get();
        while let Some(child) = self.flow.first_child() {
            self.flow.remove(&child);
        }
        self.count.set_text(&format!(
            "{}{} items",
            items.len(),
            if items.len() == 60 { "+" } else { "" }
        ));
        let rows = if self.window.width() < 700 || !self.search.text().is_empty() {
            2
        } else {
            1
        };
        self.flow.set_min_children_per_line(rows);
        self.flow.set_max_children_per_line(rows);
        *self.items.borrow_mut() = items;
        for item in self.items.borrow().iter() {
            let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
            card.add_css_class("shelf-card");
            card.set_size_request(205, 180);
            let source = label(if item.source.is_empty() {
                "Unknown application"
            } else {
                &item.source
            });
            source.set_ellipsize(gtk::pango::EllipsizeMode::End);
            source.set_max_width_chars(24);
            source.add_css_class("dim-label");
            let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let app_icon =
                gio::DesktopAppInfo::new(&format!("{}.desktop", item.source.to_lowercase()))
                    .and_then(|a| a.icon());
            let icon = if let Some(icon) = app_icon {
                gtk::Image::from_gicon(&icon)
            } else {
                gtk::Image::from_icon_name("application-x-executable-symbolic")
            };
            icon.set_pixel_size(16);
            heading.append(&icon);
            source.set_hexpand(true);
            heading.append(&source);
            let pin = gtk::ToggleButton::new();
            pin.set_icon_name(if item.pinned {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            });
            pin.set_active(item.pinned);
            pin.set_tooltip_text(Some("Pin / unpin item (P)"));
            pin.add_css_class("flat");
            let tx = self.engine.tx.clone();
            let id = item.id;
            pin.connect_toggled(move |_| {
                let _ = tx.send(Request::Pin(id));
            });
            heading.append(&pin);
            card.append(&heading);
            if let Some(bytes) = &item.thumbnail {
                if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
                    let pic = gtk::Picture::for_paintable(&texture);
                    pic.set_size_request(180, 110);
                    pic.set_can_shrink(true);
                    card.append(&pic);
                }
            } else {
                let excerpt = label(&item.preview.chars().take(220).collect::<String>());
                excerpt.set_wrap(true);
                excerpt.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                excerpt.set_max_width_chars(24);
                excerpt.set_lines(6);
                excerpt.set_ellipsize(gtk::pango::EllipsizeMode::End);
                excerpt.set_vexpand(true);
                excerpt.add_css_class("card-preview");
                card.append(&excerpt);
            }
            let age = (crate::storage::now() - item.captured).max(0);
            let stamp = if age < 60 {
                format!("{age}s")
            } else if age < 3600 {
                format!("{}m", age / 60)
            } else if age < 86400 {
                format!("{}h", age / 3600)
            } else {
                format!("{}d", age / 86400)
            };
            let info = label(&format!(
                "{} · {}{}",
                item.kind.label(),
                stamp,
                if item.pinned { " · Pinned" } else { "" }
            ));
            info.add_css_class("dim-label");
            let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            info.set_hexpand(true);
            bottom.append(&info);
            let delete = gtk::Button::from_icon_name("edit-delete-symbolic");
            delete.add_css_class("flat");
            delete.set_tooltip_text(Some("Delete item"));
            let weak = Rc::downgrade(self);
            let id = item.id;
            delete.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    s.selected.set(Some(id));
                    s.delete_selected();
                }
            });
            bottom.append(&delete);
            card.append(&bottom);
            let child = gtk::FlowBoxChild::new();
            child.set_child(Some(&card));
            child.set_focusable(true);
            child.update_property(&[gtk::accessible::Property::Label(&format!(
                "{}, {}, {}, {}{}",
                item.kind.label(),
                item.source,
                item.preview.chars().take(80).collect::<String>(),
                stamp,
                if item.pinned { ", pinned" } else { "" }
            ))]);
            self.flow.insert(&child, -1);
        }
        let index = self
            .items
            .borrow()
            .iter()
            .position(|i| Some(i.id) == selected)
            .unwrap_or(0);
        if let Some(c) = self.flow.child_at_index(index as i32) {
            self.flow.select_child(&c);
        } else {
            self.selected.set(None);
        }
    }
    fn dialog(self: &Rc<Self>, title: &str) -> gtk::Window {
        self.auxiliary.set(self.auxiliary.get() + 1);
        let w = gtk::Window::builder()
            .application(&self.app)
            .transient_for(&self.window)
            .modal(true)
            .title(title)
            .default_width(480)
            .build();
        let weak = Rc::downgrade(self);
        w.connect_destroy(move |_| {
            if let Some(s) = weak.upgrade() {
                s.auxiliary.set(s.auxiliary.get().saturating_sub(1));
            }
        });
        w
    }
    fn preview(self: &Rc<Self>) {
        let Some(id) = self.selected.get() else {
            return;
        };
        let items = self.items.borrow();
        let Some(item) = items.iter().find(|i| i.id == id) else {
            return;
        };
        let w = self.dialog("Clipboard preview");
        let text = gtk::TextView::new();
        text.set_editable(false);
        text.set_wrap_mode(gtk::WrapMode::WordChar);
        text.buffer().set_text(&item.preview);
        let sc = gtk::ScrolledWindow::builder()
            .child(&text)
            .min_content_height(300)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        if let Some(bytes) = &item.thumbnail {
            if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from(bytes)) {
                let picture = gtk::Picture::for_paintable(&texture);
                picture.set_size_request(480, 300);
                body.append(&picture);
            }
        }
        body.append(&sc);
        w.set_child(Some(&body));
        w.present();
    }
    fn delete_selected(self: &Rc<Self>) {
        let Some(id) = self.selected.get() else {
            return;
        };
        let pinned = self
            .items
            .borrow()
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.pinned)
            .unwrap_or(false);
        if !pinned {
            self.send(Request::Delete(id, false));
            return;
        }
        let weak = Rc::downgrade(self);
        self.confirm(
            "Delete pinned item?",
            "This removes the item from encrypted history.",
            move || {
                if let Some(s) = weak.upgrade() {
                    s.send(Request::Delete(id, true));
                }
            },
        );
    }
    fn confirm(self: &Rc<Self>, title: &str, message: &str, action: impl Fn() + 'static) {
        let w = self.dialog(title);
        let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
        b.set_margin_top(20);
        b.set_margin_bottom(20);
        b.set_margin_start(20);
        b.set_margin_end(20);
        let l = label(message);
        l.set_wrap(true);
        b.append(&l);
        let cancel = gtk::Button::with_label("Cancel");
        let ok = gtk::Button::with_label("Delete");
        ok.add_css_class("destructive-action");
        b.append(&cancel);
        b.append(&ok);
        let ww = w.clone();
        cancel.connect_clicked(move |_| ww.destroy());
        let ww = w.clone();
        ok.connect_clicked(move |_| {
            action();
            ww.destroy();
        });
        w.set_child(Some(&b));
        w.present();
    }
    fn confirm_clear(self: &Rc<Self>) {
        let w = self.dialog("Clear clipboard history?");
        let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
        b.set_margin_top(20);
        b.set_margin_bottom(20);
        b.set_margin_start(20);
        b.set_margin_end(20);
        b.append(&label("The current system clipboard will not change."));
        let pins = gtk::CheckButton::with_label("Also delete pinned items");
        b.append(&pins);
        let cancel = gtk::Button::with_label("Cancel");
        b.append(&cancel);
        let clear = gtk::Button::with_label("Clear history");
        clear.add_css_class("destructive-action");
        b.append(&clear);
        let ww = w.clone();
        cancel.connect_clicked(move |_| ww.destroy());
        let weak = Rc::downgrade(self);
        let ww = w.clone();
        clear.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.send(Request::Clear(pins.is_active()));
            }
            ww.destroy();
        });
        w.set_child(Some(&b));
        w.present();
    }
    /// Show editable settings and an explicit foreground-only keyring unlock.
    pub fn show_settings(self: &Rc<Self>) {
        let w = self.dialog("ClipLedge settings");
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_top(20);
        root.set_margin_bottom(20);
        root.set_margin_start(20);
        root.set_margin_end(20);
        let info=label("Clipboard history is encrypted locally. Exclusions and secret detection are best effort. Enter restores; paste in the destination application with Ctrl+V.");
        info.set_wrap(true);
        info.set_max_width_chars(65);
        root.append(&info);
        if !self.native_x11 {
            let l=label("Wayland background capture is unavailable. Window placement and activation are controlled by your compositor.");
            l.set_wrap(true);
            root.append(&l);
        }
        let unlock = gtk::Button::with_label("Unlock / Retry");
        let weak = Rc::downgrade(self);
        unlock.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.send(Request::Unlock(true));
            }
        });
        root.append(&unlock);
        let current = self.settings.borrow().clone();
        let mut checks = Vec::new();
        for (name, value) in [
            ("Capture text", current.text),
            ("Capture rich text", current.rich_text),
            ("Capture links", current.links),
            ("Capture images", current.images),
            ("Capture file references", current.files),
            ("Detect structured secrets", current.secret_detection),
            ("Show status icon", current.status_icon),
            ("Start at login", current.start_at_login),
        ] {
            let c = gtk::CheckButton::with_label(name);
            c.set_active(value);
            root.append(&c);
            checks.push(c);
        }
        root.append(&label("Excluded application identities (comma separated)"));
        let exclusions = gtk::Entry::builder()
            .text(current.exclusions.join(", "))
            .build();
        root.append(&exclusions);
        let theme = gtk::DropDown::from_strings(&["System", "Light", "Dark"]);
        theme.set_selected(
            ["System", "Light", "Dark"]
                .iter()
                .position(|t| *t == current.theme)
                .unwrap_or(0) as u32,
        );
        root.append(&label("Appearance"));
        root.append(&theme);
        let count = gtk::SpinButton::with_range(1.0, 100000.0, 1.0);
        count.set_value(current.max_items as f64);
        root.append(&label("Maximum unpinned items"));
        root.append(&count);
        let days = gtk::SpinButton::with_range(1.0, 3650.0, 1.0);
        days.set_value(current.max_days as f64);
        root.append(&label("Retention days"));
        root.append(&days);
        let size = gtk::SpinButton::with_range(1.0, 10240.0, 1.0);
        size.set_value((current.max_bytes / (1024 * 1024)) as f64);
        root.append(&label("Storage target (MiB)"));
        root.append(&size);
        let accelerator = gtk::Entry::builder()
            .text(&current.shortcut)
            .placeholder_text("Click here, then press a shortcut")
            .editable(false)
            .build();
        accelerator.set_tooltip_text(Some("Click, then press the complete key combination"));
        let capture = gtk::EventControllerKey::new();
        capture.connect_key_pressed({
            let accelerator = accelerator.clone();
            move |_, key, _, state| {
                use gdk::{Key, ModifierType};
                if [
                    Key::Alt_L,
                    Key::Alt_R,
                    Key::Control_L,
                    Key::Control_R,
                    Key::Shift_L,
                    Key::Shift_R,
                    Key::Super_L,
                    Key::Super_R,
                    Key::Meta_L,
                    Key::Meta_R,
                    Key::Hyper_L,
                    Key::Hyper_R,
                ]
                .contains(&key)
                {
                    return glib::Propagation::Stop;
                }
                let modifiers = state
                    & (ModifierType::SHIFT_MASK
                        | ModifierType::CONTROL_MASK
                        | ModifierType::ALT_MASK
                        | ModifierType::SUPER_MASK
                        | ModifierType::META_MASK
                        | ModifierType::HYPER_MASK);
                if modifiers.intersects(
                    ModifierType::CONTROL_MASK
                        | ModifierType::ALT_MASK
                        | ModifierType::SUPER_MASK
                        | ModifierType::META_MASK,
                ) {
                    accelerator.set_text(&gtk::accelerator_name(key, modifiers));
                }
                glib::Propagation::Stop
            }
        });
        accelerator.add_controller(capture);
        root.append(&label("Shortcut binding"));
        root.append(&accelerator);
        let shortcut_status = label("Click the field, then press the complete shortcut.");
        shortcut_status.add_css_class("dim-label");
        root.append(&shortcut_status);
        let install = gtk::Button::with_label("Register shortcut (check for conflicts)");
        let weak = Rc::downgrade(self);
        let shortcut_status_for_install = shortcut_status.clone();
        let accelerator_for_install = accelerator.clone();
        install.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                match desktop::install_shortcut(&accelerator_for_install.text()) {
                    Ok(()) => {
                        let mut cfg = s.settings.borrow().clone();
                        cfg.shortcut = accelerator_for_install.text().into();
                        let saved = cfg.save().is_ok();
                        *s.settings.borrow_mut() = cfg;
                        shortcut_status_for_install.set_text(if saved {
                            "Shortcut registered with Cinnamon."
                        } else {
                            "Shortcut registered, but the preference could not be saved."
                        });
                    }
                    Err(e) => shortcut_status_for_install.set_text(match e {
                        "shortcut-conflict" => "That shortcut is already in use.",
                        "shortcut-invalid" => {
                            "Press Control, Alt, Super, or Meta with another key."
                        }
                        "cinnamon-settings-unavailable" => {
                            "Cinnamon shortcut settings are unavailable."
                        }
                        _ => "Cinnamon did not save the shortcut.",
                    }),
                }
            }
        });
        root.append(&install);
        let reset = gtk::Button::with_label("Reset encrypted history and key…");
        reset.add_css_class("destructive-action");
        let weak = Rc::downgrade(self);
        reset.connect_clicked(move |_|{if let Some(s)=weak.upgrade(){let weak=Rc::downgrade(&s);s.confirm("Reset all history and its encryption key?","All history, including pins, will be removed. This cannot be undone. The system clipboard will not change.",move ||{if let Some(s)=weak.upgrade(){s.send(Request::Reset);}});}});
        root.append(&reset);
        let shortcut = gtk::Button::with_label("Configure Cinnamon shortcut…");
        let weak = Rc::downgrade(self);
        shortcut.connect_clicked(move |_| {
            if let Err(e) = desktop::keyboard_settings() {
                if let Some(s) = weak.upgrade() {
                    s.status.set_text(e);
                }
            }
        });
        root.append(&shortcut);
        let cmd=label("Custom shortcut command: clipledge --toggle\nSuggested binding: Super+V (choose another if already assigned)");
        cmd.set_selectable(true);
        root.append(&cmd);
        let save = gtk::Button::with_label("Save settings");
        save.add_css_class("suggested-action");
        root.append(&save);
        let weak = Rc::downgrade(self);
        let ww = w.clone();
        save.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                let mut cfg = s.settings.borrow().clone();
                cfg.text = checks[0].is_active();
                cfg.rich_text = checks[1].is_active();
                cfg.links = checks[2].is_active();
                cfg.images = checks[3].is_active();
                cfg.files = checks[4].is_active();
                cfg.secret_detection = checks[5].is_active();
                cfg.status_icon = checks[6].is_active();
                cfg.start_at_login = checks[7].is_active();
                cfg.exclusions = exclusions
                    .text()
                    .split(',')
                    .map(|v| v.trim().to_lowercase())
                    .filter(|v| !v.is_empty())
                    .collect();
                cfg.theme = ["System", "Light", "Dark"][theme.selected().min(2) as usize].into();
                cfg.shortcut = accelerator.text().into();
                cfg.max_items = count.value() as usize;
                cfg.max_days = days.value() as u64;
                cfg.max_bytes = size.value() as u64 * 1024 * 1024;
                cfg.onboarded = true;
                if cfg.save().is_err() || desktop::autostart(cfg.start_at_login).is_err() {
                    s.status.set_text("Unable to save settings / autostart");
                    return;
                }
                desktop::theme(&cfg.theme);
                if cfg.status_icon {
                    s.start_tray();
                } else {
                    s.tray.borrow_mut().take();
                }
                *s.settings.borrow_mut() = cfg.clone();
                s.send(Request::Settings(cfg));
                ww.destroy();
            }
        });
        let scroll = gtk::ScrolledWindow::builder()
            .child(&root)
            .min_content_height(500)
            .max_content_height(650)
            .propagate_natural_height(true)
            .build();
        w.set_child(Some(&scroll));
        w.present();
    }
    fn start_tray(&self) {
        if self.tray.borrow().is_none() {
            match crate::tray::start(&self.app, self.engine.gate.clone()) {
                Ok(t) => *self.tray.borrow_mut() = Some(t),
                Err(_) => self
                    .status
                    .set_text("Status icon unavailable; use the keyboard shortcut"),
            }
        }
    }
    fn actions(self: &Rc<Self>) {
        for name in ["show", "pause-toggle", "settings", "clear", "quit"] {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(s) = weak.upgrade() {
                    match name {
                        "show" => s.show(),
                        "pause-toggle" => s.pause(!s.engine.gate.paused.load(Ordering::SeqCst)),
                        "settings" => s.show_settings(),
                        "clear" => s.confirm_clear(),
                        "quit" => {
                            s.quit();
                        }
                        _ => {}
                    }
                }
            });
            self.app.add_action(&action);
        }
    }
    fn watch_lock(self: &Rc<Self>) {
        self.engine.gate.locked.store(true, Ordering::SeqCst);
        let bus = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
            Ok(b) => b,
            Err(_) => return,
        };
        let states = Rc::new(RefCell::new([false, false]));
        let pending = Rc::new(Cell::new(2u32));
        for (index, (service, interface, path)) in [
            (
                "org.cinnamon.ScreenSaver",
                "org.cinnamon.ScreenSaver",
                "/org/cinnamon/ScreenSaver",
            ),
            (
                "org.freedesktop.ScreenSaver",
                "org.freedesktop.ScreenSaver",
                "/org/freedesktop/ScreenSaver",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let weak = Rc::downgrade(self);
            let state = states.clone();
            bus.signal_subscribe(
                Some(service),
                Some(interface),
                Some("ActiveChanged"),
                Some(path),
                None,
                gio::DBusSignalFlags::NONE,
                move |_, _, _, _, _, args| {
                    state.borrow_mut()[index] = args.child_value(0).get::<bool>().unwrap_or(true);
                    if let Some(s) = weak.upgrade() {
                        let locked = state.borrow().iter().any(|v| *v);
                        s.engine.gate.locked.store(locked, Ordering::SeqCst);
                        s.engine.gate.invalidate();
                        if locked {
                            s.lock_history();
                        } else {
                            s.send(Request::Unlock(false));
                        }
                    }
                },
            );
            let weak = Rc::downgrade(self);
            let state = states.clone();
            let pending = pending.clone();
            bus.call(
                Some(service),
                path,
                interface,
                "GetActive",
                None,
                None,
                gio::DBusCallFlags::NO_AUTO_START,
                1000,
                gio::Cancellable::NONE,
                move |reply| {
                    if let Ok(v) = reply {
                        state.borrow_mut()[index] = v.child_value(0).get::<bool>().unwrap_or(true);
                    }
                    pending.set(pending.get().saturating_sub(1));
                    if pending.get() == 0 {
                        if let Some(s) = weak.upgrade() {
                            let locked = state.borrow().iter().any(|v| *v);
                            s.engine.gate.locked.store(locked, Ordering::SeqCst);
                            s.initializing.set(false);
                            if locked {
                                s.lock_history();
                            } else {
                                s.send(Request::Unlock(false));
                                if s.pending_show.replace(false) {
                                    s.show();
                                }
                            }
                        }
                    }
                },
            );
        }
    }
    fn watch_keyring(self: &Rc<Self>) {
        if let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
            let weak = Rc::downgrade(self);
            bus.signal_subscribe(
                Some("org.freedesktop.secrets"),
                Some("org.freedesktop.DBus.Properties"),
                Some("PropertiesChanged"),
                None,
                None,
                gio::DBusSignalFlags::NONE,
                move |_, _, _, _, _, args| {
                    let locked = args
                        .child_value(1)
                        .get::<std::collections::HashMap<String, glib::Variant>>()
                        .and_then(|v| v.get("Locked").and_then(|v| v.get::<bool>()))
                        .unwrap_or(false);
                    if locked {
                        if let Some(s) = weak.upgrade() {
                            s.lock_history();
                        }
                    }
                },
            );
            let weak = Rc::downgrade(self);
            bus.signal_subscribe(
                Some("org.freedesktop.DBus"),
                Some("org.freedesktop.DBus"),
                Some("NameOwnerChanged"),
                Some("/org/freedesktop/DBus"),
                Some("org.freedesktop.secrets"),
                gio::DBusSignalFlags::NONE,
                move |_, _, _, _, _, args| {
                    if args
                        .child_value(2)
                        .get::<String>()
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                    {
                        if let Some(s) = weak.upgrade() {
                            s.lock_history();
                        }
                    }
                },
            );
        }
    }
    fn lock_history(&self) {
        for w in self.app.windows() {
            if w != self.window.clone().upcast::<gtk::Window>() {
                w.destroy();
            }
        }
        self.engine.gate.key_ready.store(false, Ordering::SeqCst);
        self.engine.gate.invalidate();
        self.window.set_visible(false);
        self.search.set_text("");
        self.items.borrow_mut().clear();
        while let Some(child) = self.flow.first_child() {
            self.flow.remove(&child);
        }
        self.send(Request::Lock);
    }
}
