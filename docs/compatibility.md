# Compatibility and release gates

This repository produces a **development build**. Successful local tests are not
sufficient to call it a certified public release.

## Confirmed locally

- Linux Mint 22.1 libraries, Rust 1.75, GTK 4.14.5, SQLCipher 4.5.6, amd64.
- SQLCipher key verification, encrypted round trips, wrong-key rejection, pin
  invariants, cancellation, literal search, paging, and retention/index cleanup.
- Independent X11 connections under Xvfb: incremental multi-format capture,
  sensitive MIME hints, cooperative SAVE_TARGETS, source destruction, explicit
  clearing, paused capture, and another clipboard manager.
- Disposable Secret Service key creation/readback/deletion and GTK shelf rendering.
- File-reference restoration read by an independent xclip process, and text
  restoration through GDK.
- Single-instance CLI, background startup, and fail-closed missing-key behaviour.

## Deliberate compatibility adjustments

- Packaged SQLCipher provides FTS4, not FTS5. The encrypted FTS4 index is used.
- Search supports literal word-prefix queries; arbitrary mid-word substrings are
  not guaranteed by the indexed search path.
- File availability remains unknown rather than probing potential network mounts.
- Larger image previews enlarge the bounded thumbnail, not the original image.
- Preferences use private JSON; Cinnamon shortcut integration uses GSettings.

## Still required before a public release claim

- Real Cinnamon window-manager acceptance: focus return, fullscreen applications,
  workspaces, top/bottom/side panels, multiple monitors, and mixed fractional scaling.
- Firefox, Chromium, LibreOffice, Nemo, screenshot/image tools, terminals, and two
  password managers, including source identities and their sensitive hints.
- A real Wayland compositor, including XWayland detection and activation limits.
- Screen readers, high contrast, font scaling, reduced motion, and complete input
  method/keyboard checks under the real desktop.
- Robustness expansion for malformed owners, target changes during incremental
  transfers, unavailable default keyring collection, disk exhaustion, corrupt WAL,
  and keyring prompt cancellation. Current safeguards do not replace this matrix.
- Future-schema migration with encrypted backup and rollback; schema 1 only exists
  today. No speculative migration of unrelated/pre-existing databases is attempted.
- Clean installed-package upgrade/removal VM tests, ARM64 build/run verification,
  source-distribution dependency vendoring, translation catalogs, maintainer contact,
  an actual public homepage URL for AppStream,
  and signed release checksums using a release maintainer's key.
- Representative real-desktop warm activation p95 and idle/peak memory measurements.
  GTK rendering and large compaction can exceed the initial RSS/latency targets;
  no unmeasured performance claim is made.

On quit, GDK clipboard storage is attempted when another manager is available.
There is no guarantee of clipboard survival without another owner. Direct paste,
PRIMARY capture, privileged Wayland data control, global-shortcut portals on other
desktops, and Flatpak remain out of scope.
