//! Fail-closed privacy gates shared by capture and preservation.
use crate::content::Representation;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

/// Independent reasons why capture is unavailable.
#[derive(Clone, Default)]
pub struct Gate {
    pub paused: Arc<AtomicBool>,
    pub locked: Arc<AtomicBool>,
    pub key_ready: Arc<AtomicBool>,
    pub capacity: Arc<AtomicBool>,
    pub backend: Arc<AtomicBool>,
    pub epoch: Arc<AtomicU64>,
    pub ownership: Arc<AtomicU64>,
    pub dropped: Arc<AtomicU64>,
}
impl Gate {
    /// Whether a new capture may begin or be committed.
    pub fn eligible(&self) -> bool {
        !self.paused.load(Ordering::SeqCst)
            && !self.locked.load(Ordering::SeqCst)
            && self.key_ready.load(Ordering::SeqCst)
            && self.capacity.load(Ordering::SeqCst)
            && self.backend.load(Ordering::SeqCst)
    }
    /// Invalidate all in-flight operations without changing the system clipboard.
    pub fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }
    /// Capture generation used to reject stale work.
    pub fn generation(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    /// Confirm a job still belongs to the active privacy state.
    pub fn accepts(&self, epoch: u64) -> bool {
        self.eligible() && self.generation() == epoch
    }
}

/// Recognized clipboard hints that prohibit fetching or preserving an item.
pub fn sensitive_hint(mime: &str) -> bool {
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "x-kde-passwordmanagerhint"
            | "application/x-keepassxc"
            | "application/x-keepassxc-secret"
            | "application/x-clipledge-sensitive"
            | "application/x-klipper-ignore"
    )
}

/// Match exact application identities, never window titles or substrings.
pub fn excluded(source: &str, exclusions: &[String]) -> bool {
    exclusions
        .iter()
        .any(|s| s.eq_ignore_ascii_case(source.trim_end_matches(".desktop")))
}

/// Recognize high-confidence structured secrets without flagging ordinary prose.
pub fn contains_secret(reps: &[Representation]) -> bool {
    reps.iter().filter(|r| r.is_text()).any(|r| {
        let s = String::from_utf8_lossy(&r.bytes).replace('\0', "");
        [
            "-----BEGIN PRIVATE KEY-----",
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----BEGIN EC PRIVATE KEY-----",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
        ]
        .iter()
        .any(|p| s.contains(p))
            || s.split_whitespace().any(|word| {
                let w =
                    word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
                [("ghp_", 36usize), ("github_pat_", 60), ("xoxb-", 20)]
                    .iter()
                    .any(|(prefix, n)| {
                        w.strip_prefix(prefix)
                            .map(|v| {
                                v.len() >= *n
                                    && v.chars()
                                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                            })
                            .unwrap_or(false)
                    })
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resume_cannot_override_lock() {
        let g = Gate::default();
        g.key_ready.store(true, Ordering::SeqCst);
        g.capacity.store(true, Ordering::SeqCst);
        g.backend.store(true, Ordering::SeqCst);
        assert!(g.eligible());
        let old = g.generation();
        g.locked.store(true, Ordering::SeqCst);
        g.invalidate();
        g.paused.store(false, Ordering::SeqCst);
        assert!(!g.accepts(old));
    }
    #[test]
    fn precise_secret_rules() {
        let rep = |s: &str| vec![Representation::new("text/plain", s.as_bytes().to_vec())];
        assert!(contains_secret(&rep("-----BEGIN OPENSSH PRIVATE KEY-----")));
        assert!(!contains_secret(&rep(
            "Invoice 123456789 and ordinary prose"
        )));
        assert!(!excluded("my-keepassxc-notes", &["keepassxc".into()]));
    }
}
