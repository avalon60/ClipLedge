//! Core clipboard policies, encrypted persistence, and desktop adapters.
pub mod clipboard;
pub mod content;
pub mod desktop;
pub mod engine;
pub mod keyring;
pub mod security;
pub mod settings;
pub mod storage;
pub mod tray;
pub mod ui;

/// Registered application identifier.
pub const APP_ID: &str = "org.clipledge.ClipLedge";
