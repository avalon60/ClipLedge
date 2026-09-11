//! Validated user preferences and XDG paths; no clipboard content lives here.
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

/// Persistent non-sensitive settings.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub max_items: usize,
    pub max_days: u64,
    pub max_bytes: u64,
    pub text: bool,
    pub rich_text: bool,
    pub links: bool,
    pub images: bool,
    pub files: bool,
    pub secret_detection: bool,
    pub exclusions: Vec<String>,
    pub status_icon: bool,
    pub start_at_login: bool,
    pub theme: String,
    pub shortcut: String,
    pub onboarded: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            max_items: 500,
            max_days: 30,
            max_bytes: 500 * 1024 * 1024,
            text: true,
            rich_text: true,
            links: true,
            images: true,
            files: true,
            secret_detection: true,
            exclusions: vec![
                "keepassxc".into(),
                "keepass2".into(),
                "1password".into(),
                "bitwarden".into(),
            ],
            status_icon: true,
            start_at_login: false,
            theme: "System".into(),
            shortcut: "<Super>v".into(),
            onboarded: false,
        }
    }
}
/// Resolve a standard XDG directory with an absolute-path-only override.
pub fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(fallback)
        })
}
/// Location of encrypted history.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("clipledge")
}
/// Location of user preferences.
pub fn config_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("clipledge/settings.json")
}
/// Write a private file atomically, without following a destination symlink.
pub fn private_write(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path"))?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).map_err(|_| io::Error::other("random unavailable"))?;
    let temp = path.with_extension(format!("{:016x}.new", u64::from_le_bytes(nonce)));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| {
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
impl Settings {
    /// Read preferences, using safe defaults for missing or invalid settings.
    pub fn load() -> Self {
        fs::read(config_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok())
            .filter(|s| s.valid())
            .unwrap_or_default()
    }
    /// Check resource settings before committing them.
    pub fn valid(&self) -> bool {
        (1..=100_000).contains(&self.max_items)
            && (1..=3650).contains(&self.max_days)
            && (1024 * 1024..=10 * 1024 * 1024 * 1024).contains(&self.max_bytes)
            && ["System", "Light", "Dark"].contains(&self.theme.as_str())
            && !self.shortcut.is_empty()
            && self.shortcut.len() <= 128
            && !self.shortcut.chars().any(char::is_control)
    }
    /// Persist validated non-sensitive preferences.
    pub fn save(&self) -> io::Result<()> {
        if !self.valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid settings",
            ));
        }
        private_write(
            &config_path(),
            &serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )
    }
}
