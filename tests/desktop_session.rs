//! Full private-session integration against GTK, GDK, and Secret Service.
// Author: Clive Bostock
// Date: 10-Sep-2026
// Purpose: Exercise the native shelf using synthetic history in an isolated session.
// Usage: xvfb-run -a dbus-run-session --config-file=tests/session.conf -- sh tests/desktop-session.sh
use clipledge::{
    content::{prepare, Representation},
    settings::Settings,
    storage::Storage,
};
use gtk::{gio, glib, prelude::*};
use std::time::{Duration, Instant};
fn spin(duration: Duration) {
    let start = Instant::now();
    let context = glib::MainContext::default();
    while start.elapsed() < duration {
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "requires isolated X11, D-Bus, and unlocked test keyring"]
fn secret_service_and_rendered_shelf() {
    assert_eq!(std::env::var("CLIPLEDGE_TEST_ISOLATED").as_deref(), Ok("1"));
    let key = clipledge::keyring::database_key(true, false).expect("create test key");
    let again =
        clipledge::keyring::database_key(false, true).expect("read test key without prompt");
    assert_eq!(*key, *again);
    let path = clipledge::settings::data_dir().join("history.db");
    let mut db = Storage::open(&path, &key).unwrap();
    let mut cfg = Settings::default();
    cfg.onboarded = true;
    cfg.status_icon = false;
    cfg.save().unwrap();
    for (mime, text, source) in [
        (
            "text/plain",
            "Clear, practical notes for the next project. Keep useful snippets within reach.",
            "Text editor",
        ),
        ("text/plain", "https://linuxmint.com", "Firefox"),
        (
            "text/html",
            "<b>A little more focus.</b><p>A little less searching.</p>",
            "LibreOffice",
        ),
        (
            "x-special/gnome-copied-files",
            "cut\nfile:///tmp/project-notes.txt\nfile:///tmp/design-sketch.png",
            "Nemo",
        ),
    ] {
        let item = prepare(
            vec![Representation::new(mime, text.as_bytes().to_vec())],
            source.into(),
            &cfg,
        )
        .unwrap();
        db.insert(&item, &cfg).unwrap();
    }
    drop(db);
    gtk::init().unwrap();
    let app = gtk::Application::builder()
        .application_id(clipledge::APP_ID)
        .build();
    app.register(gio::Cancellable::NONE).unwrap();
    let shelf = clipledge::ui::Shelf::new(&app);
    shelf.show();
    spin(Duration::from_secs(2));
    assert!(
        shelf.diagnostics().contains("state=ready"),
        "{}",
        shelf.diagnostics()
    );
    if let Ok(path) = std::env::var("CLIPLEDGE_TEST_SCREENSHOT") {
        let window = clipledge::desktop::xid(&shelf.window)
            .map(|id| id.to_string())
            .unwrap_or_else(|| "root".into());
        assert!(std::process::Command::new("import")
            .args(["-window", &window, &path])
            .status()
            .unwrap()
            .success());
    }
    // Activate the selected file card and read through an independent X11 consumer.
    fn find_search(widget: &gtk::Widget) -> Option<gtk::SearchEntry> {
        if let Ok(entry) = widget.clone().downcast::<gtk::SearchEntry>() {
            return Some(entry);
        }
        let mut child = widget.first_child();
        while let Some(w) = child {
            if let Some(found) = find_search(&w) {
                return Some(found);
            }
            child = w.next_sibling();
        }
        None
    }
    let search = find_search(shelf.window.upcast_ref()).unwrap();
    search.emit_activate();
    spin(Duration::from_millis(200));
    let mut consumer = std::process::Command::new("xclip")
        .args([
            "-selection",
            "clipboard",
            "-o",
            "-t",
            "x-special/gnome-copied-files",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while consumer.try_wait().unwrap().is_none() && start.elapsed() < Duration::from_secs(3) {
        spin(Duration::from_millis(10));
    }
    if consumer.try_wait().unwrap().is_none() {
        consumer.kill().unwrap();
        panic!("clipboard consumer timeout");
    }
    let output = consumer.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.starts_with(b"copy\n"));
    shelf.show();
    search.set_text("practical");
    spin(Duration::from_millis(400));
    search.emit_activate();
    spin(Duration::from_millis(200));
    let text = glib::MainContext::default()
        .block_on(
            gtk::gdk::Display::default()
                .unwrap()
                .clipboard()
                .read_text_future(),
        )
        .unwrap()
        .unwrap();
    assert!(text.contains("practical notes"));
    // Exported tray properties and menu shapes must be accepted by GDBus.
    let tray = clipledge::tray::start(&app, clipledge::security::Gate::default()).unwrap();
    tray.refresh();
    spin(Duration::from_millis(50));
    drop(tray);
    shelf.pause(true);
    assert!(shelf.diagnostics().contains("capture=false"));
    shelf.stop();
    shelf.window.destroy();
    spin(Duration::from_millis(50));
    clipledge::keyring::delete_key().unwrap();
    assert!(clipledge::keyring::database_key(false, true).is_err());
}
