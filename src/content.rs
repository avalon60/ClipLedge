//! Bounded format normalization, local previews, and stable deduplication.
use crate::{security, settings::Settings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Cursor;
use unicode_normalization::UnicodeNormalization;
pub const MAX_ITEM: usize = 25 * 1024 * 1024;
pub const MAX_QUEUE: usize = 50 * 1024 * 1024;
pub const MIME_ALLOWLIST: &[&str] = &[
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain",
    "STRING",
    "text/html",
    "image/png",
    "image/jpeg",
    "image/webp",
    "text/uri-list",
    "x-special/gnome-copied-files",
];
/// An original supported clipboard representation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Representation {
    pub mime: String,
    pub bytes: Vec<u8>,
}
impl Drop for Representation {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.bytes.zeroize();
    }
}
impl Representation {
    /// Construct an owned representation.
    pub fn new(mime: &str, bytes: Vec<u8>) -> Self {
        Self {
            mime: mime.into(),
            bytes,
        }
    }
    /// Whether the format carries text that must be checked for secrets.
    pub fn is_text(&self) -> bool {
        !self.mime.starts_with("image/")
    }
}
/// Display classification used by shelf filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Text,
    RichText,
    Link,
    Image,
    Files,
}
impl Kind {
    /// Stable storage label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::RichText => "Rich text",
            Self::Link => "Link",
            Self::Image => "Image",
            Self::Files => "Files",
        }
    }
    /// Parse a stored classification.
    pub fn parse(s: &str) -> Self {
        match s {
            "Rich text" => Self::RichText,
            "Link" => Self::Link,
            "Image" => Self::Image,
            "Files" => Self::Files,
            _ => Self::Text,
        }
    }
}
/// File reference metadata; remote availability is deliberately unknown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub uri: String,
    pub name: String,
    pub cut: bool,
    pub available: Option<bool>,
}
/// A fully checked item, safe for history and automatic preservation.
#[derive(Clone)]
pub struct Capture {
    pub reps: Vec<Representation>,
    pub kind: Kind,
    pub preview: String,
    pub search: String,
    pub source: String,
    pub checksum: Vec<u8>,
    pub thumbnail: Option<Vec<u8>>,
    pub files: Vec<FileEntry>,
    pub width: u32,
    pub height: u32,
}
impl Capture {
    /// Total original payload size.
    pub fn size(&self) -> usize {
        self.reps.iter().map(|r| r.bytes.len()).sum()
    }
}
/// Normalize searchable content without changing restoration bytes.
pub fn normalize(s: &str) -> String {
    s.nfkc().flat_map(char::to_lowercase).collect()
}
/// Extract a passive text excerpt from HTML without a browser or resource loading.
fn html_text(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                tag = true;
                out.push(' ');
            }
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
}
/// Apply all privacy and category checks before generating display data.
pub fn prepare(
    mut reps: Vec<Representation>,
    source: String,
    settings: &Settings,
) -> Result<Capture, &'static str> {
    if reps.iter().map(|r| r.bytes.len()).sum::<usize>() > MAX_ITEM {
        return Err("oversized");
    }
    if reps.iter().any(|r| security::sensitive_hint(&r.mime))
        || security::excluded(&source, &settings.exclusions)
        || (settings.secret_detection && security::contains_secret(&reps))
    {
        return Err("excluded");
    }
    reps.retain(|r| MIME_ALLOWLIST.contains(&r.mime.as_str()));
    let text = reps
        .iter()
        .find(|r| {
            matches!(
                r.mime.as_str(),
                "UTF8_STRING" | "text/plain;charset=utf-8" | "text/plain" | "STRING"
            )
        })
        .map(|r| {
            if r.mime == "STRING" {
                r.bytes.iter().map(|&b| char::from(b)).collect()
            } else {
                String::from_utf8_lossy(&r.bytes).into_owned()
            }
        })
        .unwrap_or_default();
    let link = url::Url::parse(text.trim()).ok().filter(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && !text.trim().chars().any(char::is_whitespace)
    });
    if link.is_some() && !settings.links {
        return Err("category-disabled");
    }
    reps.retain(|r| match r.mime.as_str() {
        "text/html" => settings.rich_text,
        "text/uri-list" | "x-special/gnome-copied-files" => settings.files,
        m if m.starts_with("image/") => settings.images,
        _ => settings.text || link.is_some(),
    });
    if reps.is_empty() {
        return Err("unsupported");
    }
    let mut kind = if link.is_some() {
        Kind::Link
    } else if reps.iter().any(|r| r.mime == "text/html") {
        Kind::RichText
    } else {
        Kind::Text
    };
    let mut preview = if settings.text || link.is_some() {
        text.clone()
    } else {
        String::new()
    };
    if preview.is_empty() {
        if let Some(r) = reps.iter().find(|r| r.mime == "text/html") {
            preview = html_text(&String::from_utf8_lossy(&r.bytes));
        }
    }
    let mut files = Vec::new();
    if let Some(r) = reps
        .iter()
        .find(|r| r.mime == "x-special/gnome-copied-files")
        .or_else(|| reps.iter().find(|r| r.mime == "text/uri-list"))
    {
        let raw = String::from_utf8_lossy(&r.bytes);
        let cut = raw.lines().next() == Some("cut");
        for line in raw.lines().filter(|l| !l.starts_with('#')) {
            if let Ok(uri) = url::Url::parse(line) {
                let name = uri
                    .path_segments()
                    .and_then(|s| s.filter(|s| !s.is_empty()).last())
                    .unwrap_or(line)
                    .to_owned();
                // Do not stat: even file:// paths can be automounts or network filesystems.
                files.push(FileEntry {
                    uri: uri.to_string(),
                    name,
                    cut,
                    available: None,
                });
            }
        }
        if files.is_empty() {
            return Err("invalid-files");
        }
        kind = Kind::Files;
        preview = files
            .iter()
            .map(|f| f.name.clone())
            .collect::<Vec<_>>()
            .join("\n");
    }
    let (mut thumbnail, mut width, mut height) = (None, 0, 0);
    if let Some(r) = reps.iter().find(|r| r.mime.starts_with("image/")) {
        let mut reader = image::io::Reader::new(Cursor::new(&r.bytes))
            .with_guessed_format()
            .map_err(|_| "invalid-image")?;
        let dims = image::io::Reader::new(Cursor::new(&r.bytes))
            .with_guessed_format()
            .map_err(|_| "invalid-image")?
            .into_dimensions()
            .map_err(|_| "invalid-image")?;
        if u64::from(dims.0) * u64::from(dims.1) > 16_000_000 {
            return Err("image-limit");
        }
        let mut limits = image::io::Limits::default();
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let img = reader.decode().map_err(|_| "invalid-image")?;
        width = img.width();
        height = img.height();
        let mut encoded = Cursor::new(Vec::new());
        img.thumbnail(256, 160)
            .write_to(&mut encoded, image::ImageOutputFormat::Png)
            .map_err(|_| "invalid-image")?;
        if encoded.get_ref().len() > 256 * 1024 {
            return Err("thumbnail-limit");
        }
        thumbnail = Some(encoded.into_inner());
        kind = Kind::Image;
        preview = format!("{width} × {height}");
    }
    reps.sort_by(|a, b| a.mime.cmp(&b.mime));
    reps.dedup_by(|a, b| a.mime == b.mime);
    let mut hash = Sha256::new();
    for r in &reps {
        hash.update((r.mime.len() as u64).to_le_bytes());
        hash.update(r.mime.as_bytes());
        hash.update((r.bytes.len() as u64).to_le_bytes());
        hash.update(&r.bytes);
    }
    let search = normalize(&format!(
        "{} {} {}",
        preview,
        source,
        files
            .iter()
            .map(|f| f.uri.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    ));
    Ok(Capture {
        reps,
        kind,
        preview: preview.chars().take(4096).collect(),
        search,
        source,
        checksum: hash.finalize().to_vec(),
        thumbnail,
        files,
        width,
        height,
    })
}
/// Restore historical file actions as copies, never repeat a cut operation.
pub fn restoration(mut reps: Vec<Representation>) -> Vec<Representation> {
    for r in &mut reps {
        if r.mime == "x-special/gnome-copied-files" {
            let s = String::from_utf8_lossy(&r.bytes);
            r.bytes =
                format!("copy\n{}", s.lines().skip(1).collect::<Vec<_>>().join("\n")).into_bytes();
        }
    }
    reps
}
#[cfg(test)]
mod tests {
    use super::*;
    fn r(m: &str, s: &str) -> Representation {
        Representation::new(m, s.as_bytes().to_vec())
    }
    #[test]
    fn categories_do_not_leak_links() {
        let mut s = Settings::default();
        s.links = false;
        assert!(prepare(vec![r("text/plain", "https://example.com")], "".into(), &s).is_err());
    }
    #[test]
    fn rich_secret_rejects_entire_item() {
        assert!(prepare(
            vec![
                r("text/plain", "innocent"),
                r("text/html", "-----BEGIN PRIVATE KEY-----")
            ],
            "".into(),
            &Settings::default()
        )
        .is_err());
    }
    #[test]
    fn cut_becomes_copy() {
        let a = restoration(vec![r(
            "x-special/gnome-copied-files",
            "cut\nfile:///tmp/example",
        )]);
        assert_eq!(a[0].bytes, b"copy\nfile:///tmp/example");
    }
    #[test]
    fn deterministic_digest() {
        let a = prepare(
            vec![r("text/plain", "one"), r("UTF8_STRING", "one")],
            "a".into(),
            &Settings::default(),
        )
        .unwrap();
        let b = prepare(
            vec![r("UTF8_STRING", "one"), r("text/plain", "one")],
            "b".into(),
            &Settings::default(),
        )
        .unwrap();
        assert_eq!(a.checksum, b.checksum);
    }
    #[test]
    fn normalization() {
        assert_eq!(normalize("ＡbC"), "abc");
    }
}
