# Privacy

History is local and encrypted at rest with SQLCipher. The directory is mode
0700; database and settings files are mode 0600. There is no plaintext fallback.
A random 256-bit key is stored in the session's Secret Service and read back
before persistent history is created. Existing history with a missing, wrong,
or ambiguous key remains intact until an explicitly confirmed reset.

The Secret Service plain session method is used over the user's local D-Bus
session. This is not protection against malicious processes running as the same
user. SQLCipher does not protect clipboard data while the session is unlocked,
and the live clipboard remains accessible according to the display server's
rules. Core dumps are disabled; key and representation buffers are zeroized where
owned by Rust. GTK, D-Bus, and operating-system copies cannot be guaranteed erased.

Application exclusions use exact identities when the selection owner exposes
one. Unknown source identities are permitted. Recognized sensitive MIME hints
and structured private-key/token patterns reject the entire item before preview,
indexing, persistence, or automatic preservation. Detection is best effort and
must not be treated as a password-vault boundary.

Pause, screen lock, loss of the keyring, and storage pressure prevent new capture.
There is no paused-capture backlog. A previously restored live clipboard provider
may remain, because pausing or locking history does not silently clear the system
clipboard. Source destruction can be preserved only after an eligible capture
completed; explicit clearing is never resurrected.

HTML is retained for restoration but previewed as passive text. Links are never
fetched. Images are decoded locally under byte, pixel, and allocation limits.
File references are not opened or mounted automatically, and restored cuts become
copies. Previewing never reads file contents.

Deletion removes application-managed history and reconstructs the FTS index so
old index entries are not retained as tombstones. Clear History may retain pins;
Reset removes pins and the Secret Service key too. Neither operation erases copies
in external backups, filesystem snapshots, storage firmware, or other clipboard
managers. Package removal preserves user data and the key.

Diagnostics contain state labels, capabilities, and versions, not clipboard text,
search terms, digests, filenames, or URLs. No cloud synchronization, telemetry,
crash upload, URL metadata, or automatic update request is implemented.
