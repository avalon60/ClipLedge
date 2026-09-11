//! Native Cinnamon clipboard history application.
// Author: Clive Bostock
// Date: 10-Sep-2026
// Purpose: Run the encrypted, keyboard-first ClipLedge clipboard manager.
// Usage: clipledge --toggle (or --background, --settings, --status)
use gtk::{gio, glib, prelude::*};
use std::{cell::RefCell, rc::Rc};
fn main() -> glib::ExitCode {
    // Clipboard plaintext must not be written into a process core dump.
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &limit);
        libc::umask(0o077);
    }
    // Fast remote invocations do not initialize GTK or activate desktop portals.
    let args: Vec<String> = std::env::args().collect();
    let running = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .ok()
        .and_then(|bus| {
            bus.call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                Some(&(clipledge::APP_ID,).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
            .ok()
        })
        .and_then(|v| v.get::<(bool,)>())
        .map(|v| v.0)
        .unwrap_or(false);
    if running {
        let remote = gio::Application::new(
            Some(clipledge::APP_ID),
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::IS_LAUNCHER,
        );
        add_options(&remote);
        return remote.run();
    }
    if args.iter().any(|s| s == "--status") {
        println!("running=false\nhistory=not-opened");
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|s| s == "--quit") {
        return glib::ExitCode::SUCCESS;
    }
    let app = gtk::Application::builder()
        .application_id(clipledge::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    add_options(&app);
    let state: Rc<RefCell<Option<Rc<clipledge::ui::Shelf>>>> = Rc::new(RefCell::new(None));
    let s = state.clone();
    app.connect_command_line(move |app, cmd| {
        let options = cmd.options_dict();
        if options.contains("status") {
            let text = if let Some(ui) = s.borrow().as_ref() {
                ui.diagnostics()
            } else {
                "running=false\nhistory=not-opened\n".into()
            };
            {
                use glib::translate::ToGlibPtr;
                let text = std::ffi::CString::new(text).unwrap();
                unsafe {
                    gio::ffi::g_application_command_line_print(
                        cmd.to_glib_none().0,
                        b"%s\0".as_ptr().cast(),
                        text.as_ptr(),
                    );
                }
            }
            return 0;
        }
        if options.contains("quit") && s.borrow().is_none() {
            return 0;
        }
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(clipledge::ui::Shelf::new(app));
        }
        let ui = s.borrow().as_ref().unwrap().clone();
        if options.contains("quit") {
            ui.quit();
        } else if options.contains("pause") {
            ui.pause(true);
        } else if options.contains("resume") {
            ui.pause(false);
        } else if options.contains("hide") {
            ui.window.set_visible(false);
        } else if options.contains("settings") {
            ui.show_settings();
        } else if options.contains("toggle") {
            if ui.window.is_visible() {
                ui.window.set_visible(false);
            } else {
                ui.show();
            }
        } else if !options.contains("background") {
            ui.show();
        }
        0
    });
    let s = state.clone();
    app.connect_activate(move |app| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(clipledge::ui::Shelf::new(app));
        }
        s.borrow().as_ref().unwrap().show();
    });
    let s = state.clone();
    app.connect_shutdown(move |_| {
        if let Some(ui) = s.borrow().as_ref() {
            ui.stop();
        }
    });
    app.run()
}

/// Register the same command vocabulary for primary and lightweight remote clients.
fn add_options(app: &impl IsA<gio::Application>) {
    for name in [
        "show",
        "toggle",
        "hide",
        "pause",
        "resume",
        "settings",
        "status",
        "quit",
        "background",
    ] {
        app.add_main_option(
            name,
            glib::Char::from(0),
            glib::OptionFlags::NONE,
            glib::OptionArg::None,
            &format!("{name} ClipLedge"),
            None,
        );
    }
}
