//! Cinnamon autostart, conservative X11 placement, and shortcut integration.
use crate::settings;
use gtk::{gdk, prelude::*};
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::{
    connection::Connection,
    protocol::{randr::ConnectionExt as _, xproto::*},
};
/// Active X11 application and work area recorded before presenting the shelf.
#[derive(Clone, Copy, Default)]
pub struct Position {
    pub previous: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
}
/// Record the active application and choose its monitor, falling back to pointer.
pub fn position() -> Position {
    let Ok((c, s)) = x11rb::connect(None) else {
        return Position {
            width: 1000,
            ..Default::default()
        };
    };
    let root = c.setup().roots[s].root;
    let atom = |n: &str| {
        c.intern_atom(false, n.as_bytes())
            .ok()
            .and_then(|r| r.reply().ok())
            .map(|r| r.atom)
            .unwrap_or(0)
    };
    let previous = c
        .get_property(
            false,
            root,
            atom("_NET_ACTIVE_WINDOW"),
            AtomEnum::WINDOW,
            0,
            1,
        )
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|r| r.value32().and_then(|mut v| v.next()))
        .unwrap_or(0);
    let point = if previous != 0 {
        c.get_geometry(previous)
            .ok()
            .and_then(|r| r.reply().ok())
            .and_then(|g| {
                c.translate_coordinates(previous, root, (g.width / 2) as i16, (g.height / 2) as i16)
                    .ok()
                    .and_then(|r| r.reply().ok())
                    .map(|r| (i32::from(r.dst_x), i32::from(r.dst_y)))
            })
    } else {
        None
    }
    .or_else(|| {
        c.query_pointer(root)
            .ok()
            .and_then(|r| r.reply().ok())
            .map(|r| (i32::from(r.root_x), i32::from(r.root_y)))
    })
    .unwrap_or((0, 0));
    let mut area = (
        0,
        0,
        i32::from(c.setup().roots[s].width_in_pixels),
        i32::from(c.setup().roots[s].height_in_pixels),
    );
    if let Some(monitors) = c
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|r| r.reply().ok())
    {
        if let Some(m) = monitors
            .monitors
            .iter()
            .find(|m| {
                point.0 >= i32::from(m.x)
                    && point.0 < i32::from(m.x) + i32::from(m.width)
                    && point.1 >= i32::from(m.y)
                    && point.1 < i32::from(m.y) + i32::from(m.height)
            })
            .or_else(|| monitors.monitors.iter().find(|m| m.primary))
        {
            area = (m.x.into(), m.y.into(), m.width.into(), m.height.into());
        }
    }
    // Intersect with the current desktop's EWMH work area, excluding panels.
    let desk = c
        .get_property(
            false,
            root,
            atom("_NET_CURRENT_DESKTOP"),
            AtomEnum::CARDINAL,
            0,
            1,
        )
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|r| r.value32().and_then(|mut v| v.next()))
        .unwrap_or(0);
    if let Some(v) = c
        .get_property(
            false,
            root,
            atom("_NET_WORKAREA"),
            AtomEnum::CARDINAL,
            desk * 4,
            4,
        )
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|r| r.value32().map(|v| v.collect::<Vec<_>>()))
    {
        if v.len() == 4 {
            let right = (area.0 + area.2).min((v[0] + v[2]) as i32);
            let bottom = (area.1 + area.3).min((v[1] + v[3]) as i32);
            area.0 = area.0.max(v[0] as i32);
            area.1 = area.1.max(v[1] as i32);
            area.2 = (right - area.0).max(200);
            area.3 = (bottom - area.1).max(100);
        }
    }
    let width = (area.2 * 92 / 100).min(1100);
    Position {
        previous,
        x: area.0 + (area.2 - width) / 2,
        y: area.1 + 20,
        width,
    }
}
#[link(name = "gtk-4")]
extern "C" {
    fn gdk_x11_surface_get_xid(surface: *mut std::ffi::c_void) -> u64;
}
/// Obtain an XID only for native X11 surfaces.
pub fn xid(window: &gtk::ApplicationWindow) -> Option<u32> {
    use gtk::glib::translate::ToGlibPtr;
    let surface = window.surface()?;
    if !surface.type_().name().contains("X11") {
        return None;
    }
    Some(unsafe {
        gdk_x11_surface_get_xid(
            <gdk::Surface as ToGlibPtr<*mut gdk::ffi::GdkSurface>>::to_glib_none(&surface)
                .0
                .cast(),
        )
    } as u32)
}
/// Move a realized X11 shelf using the native backend.
pub fn place(window: &gtk::ApplicationWindow, p: Position) {
    if let (Some(id), Ok((c, _))) = (xid(window), x11rb::connect(None)) {
        let _ = c.configure_window(id, &ConfigureWindowAux::new().x(p.x).y(p.y));
        let _ = c.flush();
    }
}
/// Request focus return only if focus is still on this shelf (or nowhere).
pub fn return_focus(window: &gtk::ApplicationWindow, p: Position) {
    let (Some(id), Ok((c, s))) = (xid(window), x11rb::connect(None)) else {
        return;
    };
    if p.previous == 0 {
        return;
    }
    let root = c.setup().roots[s].root;
    let Some(atom) = c
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .ok()
        .and_then(|r| r.reply().ok())
        .map(|r| r.atom)
    else {
        return;
    };
    let active = c
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|r| r.value32().and_then(|mut v| v.next()))
        .unwrap_or(0);
    if active != id && active != 0 {
        return;
    }
    if c.get_window_attributes(p.previous)
        .ok()
        .and_then(|r| r.reply().ok())
        .is_none()
    {
        return;
    }
    let event = ClientMessageEvent::new(32, p.previous, atom, [1, 0, id, 0, 0]);
    let _ = c.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    );
    let _ = c.flush();
}
/// Install or remove only ClipLedge's user autostart entry.
pub fn autostart(enabled: bool) -> std::io::Result<()> {
    let path = settings::xdg("XDG_CONFIG_HOME", ".config")
        .join("autostart/org.clipledge.ClipLedge.desktop");
    if enabled {
        settings::private_write(
            &path,
            include_bytes!("../data/org.clipledge.ClipLedge-autostart.desktop"),
        )
    } else {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}
/// Open Cinnamon's native keyboard settings without modifying existing shortcuts.
pub fn keyboard_settings() -> Result<(), &'static str> {
    std::process::Command::new("cinnamon-settings")
        .arg("keyboard")
        .spawn()
        .map(|_| ())
        .map_err(|_| "keyboard-settings-unavailable")
}
/// Whether the actual backend is native X11, excluding XWayland sessions.
pub fn native_x11(display: &gdk::Display) -> bool {
    display.type_().name().contains("X11")
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
        && std::env::var("XDG_SESSION_TYPE").unwrap_or_default() != "wayland"
}
/// Apply the user's GTK appearance override.
pub fn theme(value: &str) {
    if let Some(s) = gtk::Settings::default() {
        match value {
            "Dark" => s.set_gtk_application_prefer_dark_theme(true),
            "Light" => s.set_gtk_application_prefer_dark_theme(false),
            _ => {
                s.reset_property("gtk-application-prefer-dark-theme");
            }
        }
    }
}

/// Register a user-requested Cinnamon shortcut without replacing another binding.
pub fn install_shortcut(binding: &str) -> Result<(), &'static str> {
    use gtk::gio;
    let wanted = gtk::accelerator_parse(binding).ok_or("shortcut-invalid")?;
    let source = gio::SettingsSchemaSource::default().ok_or("cinnamon-settings-unavailable")?;
    let base_schema = source
        .lookup("org.cinnamon.desktop.keybindings", true)
        .ok_or("cinnamon-settings-unavailable")?;
    let custom_schema = source
        .lookup("org.cinnamon.desktop.keybindings.custom-keybinding", true)
        .ok_or("cinnamon-settings-unavailable")?;
    let base = gio::Settings::new_full(&base_schema, gio::SettingsBackend::NONE, None);
    let names = base.strv("custom-list");
    for name in &names {
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("shortcut-config-invalid");
        }
        let path = format!("/org/cinnamon/desktop/keybindings/custom-keybindings/{name}/");
        let other =
            gio::Settings::new_full(&custom_schema, gio::SettingsBackend::NONE, Some(&path));
        if name.as_str() == "clipledge" && other.string("command") == "clipledge --toggle" {
            continue;
        }
        if other
            .strv("binding")
            .iter()
            .any(|s| gtk::accelerator_parse(s.as_str()).as_ref() == Some(&wanted))
        {
            return Err("shortcut-conflict");
        }
    }
    for id in source
        .list_schemas(true)
        .0
        .iter()
        .filter(|s| s.starts_with("org.cinnamon"))
    {
        if let Some(schema) = source.lookup(id, true) {
            if schema.path().is_some() {
                let settings = gio::Settings::new_full(&schema, gio::SettingsBackend::NONE, None);
                for key in schema.list_keys() {
                    let value = settings.value(&key);
                    if let Some(values) = value.get::<Vec<String>>() {
                        if values
                            .iter()
                            .any(|v| gtk::accelerator_parse(v).as_ref() == Some(&wanted))
                        {
                            return Err("shortcut-conflict");
                        }
                    }
                }
            }
        }
    }
    let custom = gio::Settings::new_full(
        &custom_schema,
        gio::SettingsBackend::NONE,
        Some("/org/cinnamon/desktop/keybindings/custom-keybindings/clipledge/"),
    );
    if !custom.string("command").is_empty() && custom.string("command") != "clipledge --toggle" {
        return Err("shortcut-conflict");
    }
    custom.delay();
    custom
        .set_string("name", "ClipLedge")
        .map_err(|_| "shortcut-write")?;
    custom
        .set_string("command", "clipledge --toggle")
        .map_err(|_| "shortcut-write")?;
    custom
        .set_strv("binding", [binding])
        .map_err(|_| "shortcut-write")?;
    custom.apply();
    if !names.iter().any(|s| s.as_str() == "clipledge") {
        let mut updated = names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        updated.push("clipledge".into());
        base.set_strv("custom-list", updated)
            .map_err(|_| "shortcut-write")?;
    }
    gio::Settings::sync();
    if custom.string("name") != "ClipLedge"
        || custom.string("command") != "clipledge --toggle"
        || custom.strv("binding").as_slice() != [binding]
        || !base
            .strv("custom-list")
            .iter()
            .any(|s| s.as_str() == "clipledge")
    {
        return Err("shortcut-write");
    }
    Ok(())
}
