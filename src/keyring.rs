//! Secret Service access with explicit foreground-only unlock prompting.
use glib::{prelude::*, variant::ObjectPath};
use gtk::{gio, glib};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
const SERVICE: &str = "org.freedesktop.secrets";
const ROOT: &str = "/org/freedesktop/secrets";
fn call(
    bus: &gio::DBusConnection,
    path: &str,
    interface: &str,
    method: &str,
    args: glib::Variant,
) -> Result<glib::Variant, &'static str> {
    bus.call_sync(
        Some(SERVICE),
        path,
        interface,
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
    )
    .map_err(|_| "keyring-unavailable")
}
fn unlock(
    bus: &gio::DBusConnection,
    objects: Vec<ObjectPath>,
    interactive: bool,
) -> Result<(), &'static str> {
    if !interactive {
        return Err("keyring-locked");
    }
    let reply = call(
        bus,
        ROOT,
        "org.freedesktop.Secret.Service",
        "Unlock",
        (objects,).to_variant(),
    )?;
    let (_, prompt): (Vec<ObjectPath>, ObjectPath) = reply.get().ok_or("keyring-protocol")?;
    if prompt.as_str() == "/" {
        return Ok(());
    }
    let done = Rc::new(RefCell::new(None));
    let result = done.clone();
    let sub = bus.signal_subscribe(
        Some(SERVICE),
        Some("org.freedesktop.Secret.Prompt"),
        Some("Completed"),
        Some(prompt.as_str()),
        None,
        gio::DBusSignalFlags::NONE,
        move |_, _, _, _, _, v| {
            let dismissed = v.child_value(0).get::<bool>().unwrap_or(true);
            *result.borrow_mut() = Some(!dismissed);
        },
    );
    let issued = call(
        bus,
        prompt.as_str(),
        "org.freedesktop.Secret.Prompt",
        "Prompt",
        ("",).to_variant(),
    );
    let start = Instant::now();
    let context = glib::MainContext::ref_thread_default();
    while issued.is_ok() && done.borrow().is_none() && start.elapsed() < Duration::from_secs(60) {
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bus.signal_unsubscribe(sub);
    if *done.borrow() == Some(true) {
        Ok(())
    } else {
        Err("keyring-locked")
    }
}
/// Retrieve the database key; create it only when no database exists.
///
/// Background calls never unlock collections or display prompts.
pub fn database_key(interactive: bool, existing: bool) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| load(interactive, existing))
        .map_err(|_| "keyring-context")?
}
fn load(interactive: bool, existing: bool) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|_| "keyring-unavailable")?;
    let attrs = HashMap::from([
        ("application", "org.clipledge.ClipLedge"),
        ("purpose", "history-key"),
    ]);
    let found = call(
        &bus,
        ROOT,
        "org.freedesktop.Secret.Service",
        "SearchItems",
        (&attrs,).to_variant(),
    )?;
    let (open, locked): (Vec<ObjectPath>, Vec<ObjectPath>) =
        found.get().ok_or("keyring-protocol")?;
    if open.len() + locked.len() > 1 {
        return Err("keyring-ambiguous");
    }
    if !locked.is_empty() {
        unlock(&bus, locked.clone(), interactive)?;
    }
    let session = call(
        &bus,
        ROOT,
        "org.freedesktop.Secret.Service",
        "OpenSession",
        ("plain", "".to_variant()).to_variant(),
    )?;
    let (_, session): (glib::Variant, ObjectPath) = session.get().ok_or("keyring-protocol")?;
    let outcome = (|| {
        if let Some(item) = open.first().or(locked.first()) {
            return read_secret(&bus, item, &session);
        }
        if existing {
            return Err("key-missing-existing-history");
        }
        // Creation is a deliberate first-run action, never a background side effect.
        if !interactive {
            return Err("keyring-setup-required");
        }
        let alias = call(
            &bus,
            ROOT,
            "org.freedesktop.Secret.Service",
            "ReadAlias",
            ("default",).to_variant(),
        )?;
        let (collection,): (ObjectPath,) = alias.get().ok_or("keyring-protocol")?;
        if collection.as_str() == "/" {
            return Err("keyring-no-default-collection");
        }
        unlock(&bus, vec![collection.clone()], true)?;
        let mut key = Zeroizing::new(vec![0u8; 32]);
        getrandom::getrandom(&mut key).map_err(|_| "random-unavailable")?;
        let properties = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label",
                "ClipLedge encrypted history".to_variant(),
            ),
            ("org.freedesktop.Secret.Item.Attributes", attrs.to_variant()),
        ]);
        let secret = (
            session.clone(),
            Vec::<u8>::new(),
            key.to_vec(),
            "application/octet-stream".to_owned(),
        );
        let reply = call(
            &bus,
            collection.as_str(),
            "org.freedesktop.Secret.Collection",
            "CreateItem",
            (properties, secret, false).to_variant(),
        )?;
        let (item, prompt): (ObjectPath, ObjectPath) = reply.get().ok_or("keyring-protocol")?;
        if prompt.as_str() != "/" {
            return Err("keyring-creation-prompt-required");
        }
        let readback = read_secret(&bus, &item, &session)?;
        if *readback != *key {
            return Err("keyring-verification");
        }
        Ok(key)
    })();
    let _ = call(
        &bus,
        session.as_str(),
        "org.freedesktop.Secret.Session",
        "Close",
        ().to_variant(),
    );
    outcome
}
fn read_secret(
    bus: &gio::DBusConnection,
    item: &ObjectPath,
    session: &ObjectPath,
) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let v = call(
        bus,
        item.as_str(),
        "org.freedesktop.Secret.Item",
        "GetSecret",
        (session,).to_variant(),
    )?;
    let ((_, _, value, _),): ((ObjectPath, Vec<u8>, Vec<u8>, String),) =
        v.get().ok_or("keyring-protocol")?;
    if value.len() != 32 {
        return Err("keyring-invalid-key");
    }
    Ok(Zeroizing::new(value))
}

/// Delete only ClipLedge's key after an explicit destructive-reset confirmation.
pub fn delete_key() -> Result<(), &'static str> {
    let context = glib::MainContext::new();
    context
        .with_thread_default(|| {
            let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
                .map_err(|_| "keyring-unavailable")?;
            let attrs = HashMap::from([
                ("application", "org.clipledge.ClipLedge"),
                ("purpose", "history-key"),
            ]);
            let v = call(
                &bus,
                ROOT,
                "org.freedesktop.Secret.Service",
                "SearchItems",
                (attrs,).to_variant(),
            )?;
            let (open, locked): (Vec<ObjectPath>, Vec<ObjectPath>) =
                v.get().ok_or("keyring-protocol")?;
            if !locked.is_empty() {
                unlock(&bus, locked.clone(), true)?;
            }
            for item in open.into_iter().chain(locked) {
                let v = call(
                    &bus,
                    item.as_str(),
                    "org.freedesktop.Secret.Item",
                    "Delete",
                    ().to_variant(),
                )?;
                let (prompt,): (ObjectPath,) = v.get().ok_or("keyring-protocol")?;
                if prompt.as_str() != "/" {
                    return Err("keyring-delete-prompt-required");
                }
            }
            Ok(())
        })
        .map_err(|_| "keyring-context")?
}
