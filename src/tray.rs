//! StatusNotifierItem and DBusMenu exports using GIO, without GTK 3.
use gtk::{gio, glib, prelude::*};
use std::{collections::HashMap, rc::Rc};
const ITEM: &str = r#"<node><interface name="org.kde.StatusNotifierItem"><method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method><method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method><method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method><property name="Category" type="s" access="read"/><property name="Id" type="s" access="read"/><property name="Title" type="s" access="read"/><property name="Status" type="s" access="read"/><property name="IconName" type="s" access="read"/><property name="ItemIsMenu" type="b" access="read"/><property name="Menu" type="o" access="read"/><signal name="NewIcon"/><signal name="NewStatus"><arg type="s"/></signal></interface></node>"#;
const MENU: &str = r#"<node><interface name="com.canonical.dbusmenu"><method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/><arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method><method name="GetGroupProperties"><arg type="ai" direction="in"/><arg type="as" direction="in"/><arg type="a(ia{sv})" direction="out"/></method><method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method><method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method><property name="Version" type="u" access="read"/><property name="TextDirection" type="s" access="read"/><property name="Status" type="s" access="read"/><property name="IconThemePath" type="as" access="read"/></interface></node>"#;
/// Registered tray objects; dropping removes them from the session bus.
pub struct Tray {
    bus: gio::DBusConnection,
    ids: Vec<gio::RegistrationId>,
    watch: Option<Box<dyn FnOnce()>>,
}
impl Drop for Tray {
    fn drop(&mut self) {
        if let Some(w) = self.watch.take() {
            w();
        }
        for id in self.ids.drain(..) {
            let _ = self.bus.unregister_object(id);
        }
    }
}
fn properties(id: i32) -> HashMap<String, glib::Variant> {
    let label = match id {
        1 => "Show Shelf",
        2 => "Pause / Resume Capture",
        3 => "Settings",
        4 => "Clear History",
        5 => "Quit",
        _ => "",
    };
    HashMap::from([
        ("label".into(), label.to_variant()),
        ("enabled".into(), true.to_variant()),
        ("visible".into(), true.to_variant()),
    ])
}
/// Export a status icon and register whenever a watcher becomes available.
pub fn start(app: &gtk::Application, gate: crate::security::Gate) -> Result<Tray, glib::Error> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let info = gio::DBusNodeInfo::for_xml(ITEM)?;
    let app2 = app.clone();
    let gate2 = gate.clone();
    let id = bus.register_object(
        "/StatusNotifierItem",
        &info.interfaces()[0],
        move |_, _, _, _, method, _, inv| {
            if matches!(method, "Activate" | "SecondaryActivate" | "ContextMenu") {
                app2.activate_action("show", None);
            }
            inv.return_value(Some(&().to_variant()));
        },
        move |_, _, _, _, p| match p {
            "Category" => "ApplicationStatus".to_variant(),
            "Id" => "clipledge".to_variant(),
            "Title" => "ClipLedge".to_variant(),
            "Status" => "Active".to_variant(),
            "IconName" => if !gate2.key_ready.load(std::sync::atomic::Ordering::SeqCst) {
                "dialog-password-symbolic"
            } else if gate2.paused.load(std::sync::atomic::Ordering::SeqCst) {
                "media-playback-pause-symbolic"
            } else {
                "edit-paste-symbolic"
            }
            .to_variant(),
            "ItemIsMenu" => false.to_variant(),
            "Menu" => glib::variant::ObjectPath::try_from("/Menu")
                .unwrap()
                .to_variant(),
            _ => "".to_variant(),
        },
        |_, _, _, _, _, _| false,
    )?;
    let info = gio::DBusNodeInfo::for_xml(MENU)?;
    let app2 = app.clone();
    let menu = bus.register_object(
        "/Menu",
        &info.interfaces()[0],
        move |_, _, _, _, method, args, inv| match method {
            "GetLayout" => {
                let children: Vec<glib::Variant> = (1..=5)
                    .map(|id| (id, properties(id), Vec::<glib::Variant>::new()).to_variant())
                    .collect();
                let root = (
                    0i32,
                    HashMap::from([("children-display".to_owned(), "submenu".to_variant())]),
                    children,
                );
                inv.return_value(Some(&(1u32, root).to_variant()));
            }
            "GetGroupProperties" => {
                let ids = args.child_value(0).get::<Vec<i32>>().unwrap_or_default();
                let props: Vec<_> = ids.into_iter().map(|id| (id, properties(id))).collect();
                inv.return_value(Some(&(props,).to_variant()));
            }
            "AboutToShow" => inv.return_value(Some(&(false,).to_variant())),
            "Event" => {
                let id = args.child_value(0).get::<i32>().unwrap_or(0);
                let name = args.child_value(1).get::<String>().unwrap_or_default();
                if name == "clicked" {
                    let action = match id {
                        1 => "show",
                        2 => "pause-toggle",
                        3 => "settings",
                        4 => "clear",
                        5 => "quit",
                        _ => "",
                    };
                    if !action.is_empty() {
                        app2.activate_action(action, None);
                    }
                }
                inv.return_value(Some(&().to_variant()));
            }
            _ => {
                inv.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unknown method")
            }
        },
        |_, _, _, _, p| match p {
            "Version" => 3u32.to_variant(),
            "TextDirection" => "ltr".to_variant(),
            "Status" => "normal".to_variant(),
            "IconThemePath" => Vec::<String>::new().to_variant(),
            _ => "".to_variant(),
        },
        |_, _, _, _, _, _| false,
    )?;
    let b = Rc::new(bus.clone());
    let watch = gio::bus_watch_name_on_connection(
        &bus,
        "org.kde.StatusNotifierWatcher",
        gio::BusNameWatcherFlags::NONE,
        move |_, _, _| {
            b.call(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                "org.kde.StatusNotifierWatcher",
                "RegisterStatusNotifierItem",
                Some(&("/StatusNotifierItem",).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                2000,
                gio::Cancellable::NONE,
                |_| {},
            );
        },
        |_, _| {},
    );
    Ok(Tray {
        bus,
        ids: vec![id, menu],
        watch: Some(Box::new(move || gio::bus_unwatch_name(watch))),
    })
}
impl Tray {
    /// Ask hosts to refresh the privacy/status icon.
    pub fn refresh(&self) {
        let _ = self.bus.emit_signal(
            None,
            "/StatusNotifierItem",
            "org.kde.StatusNotifierItem",
            "NewIcon",
            None,
        );
    }
}
