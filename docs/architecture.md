# Architecture

One `gtk::Application` owns the GTK window, clipboard provider, status icon,
and CLI actions. A hold guard keeps it alive with no visible windows. Later
invocations route through GApplication's unique D-Bus identity.

## Data flow

1. The X11 thread observes XFixes ownership notifications, retrieves an explicit
   target allowlist, and enforces transfer time and byte limits. Sensitive MIME
   hints reject the entire offer before content retrieval.
2. A single CPU worker checks source exclusions and structured secrets, filters
   categories, normalizes search data, hashes representations, and decodes images.
3. The writer checks the privacy epoch again and transactionally stores eligible
   data. A final epoch check prevents cancelled work from committing.
4. Only accepted captures are returned to the X11 preservation state machine.
   Cooperative SAVE_TARGETS and source destruction request GDK publication.
5. GTK owns published content until replaced or the application exits. Own-owner
   tracking and ownership generations suppress recapture. Private markers are
   supplemental, not the only suppression mechanism.

The raw and prepared queues each hold at most one 25 MiB item; the worker owns
at most one in-flight decode. Queries use an independent keyed, query-only
connection. Restoring uses that reader directly; timestamp updates are serialized
through the writer. All GTK/GDK objects remain on the main thread.

## Storage

Schema version 1 contains items, supported representations, file metadata,
thumbnail BLOBs, an FTS4 index, and completed schema records. Newer schema versions
are rejected without replacing the database. There is currently no legacy schema
to migrate; a future migration must add the planned encrypted backup/rollback
implementation and upgrade tests before increasing the version.

The database uses a native sqlite3_key call against system SQLCipher. Every
connection verifies cipher availability, cipher memory security, memory-only
temporary storage, keyed schema access, and integrity. WAL contains encrypted
payload pages. FTS4 is necessary for the actual Mint 22.1 SQLCipher build.

Literal search uses quoted alphanumeric word prefixes to narrow candidate IDs,
then escaped literal matching. Untrusted search text cannot supply FTS syntax.
Pages contain no original clipboard BLOBs and are limited to 60 results.

Retention applies age/count rules to unpinned rows, then logical and physical
storage limits. Checkpoints reclaim WAL space; compaction prunes unpinned rows
only. Disk-space checks reserve headroom. Large VACUUM operations can still
briefly lock the database; the UI retains its current page and reports a busy
read rather than blocking GTK.

## Desktop adapters

Cinnamon shortcuts use its installed GSettings schemas and never replace an
unrelated binding. App preferences use a private atomic JSON file. User autostart
uses an XDG desktop entry. The status icon implements StatusNotifierItem and
DBusMenu directly with GIO and re-registers after watcher restart.

Lock signals, initial screen-lock probes, and Secret Service loss/lock events
invalidate the capture epoch and close history connections. If neither supported
screen-saver service exists, no active lock can be established; this environment
needs manual compatibility validation before a support claim.
