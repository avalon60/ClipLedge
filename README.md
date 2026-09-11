# ClipLedge

`ClipLedge` is a native Rust/GTK 4 clipboard history application for Linux Mint
Cinnamon. It keeps supported clipboard items in a local encrypted database and
shows them as a compact, searchable shelf.

> **The key interaction:** Enter restores the selected item to the clipboard.
> Return to your application and press Ctrl+V to paste it. `ClipLedge` never
> types into another application or injects a paste keystroke.

This is currently a **development build**, not a certified public release. Native
Cinnamon/X11 is the supported development target. See [Compatibility and release
gates](docs/compatibility.md) for the remaining validation work.

## Install the development package

The repository currently contains an amd64 Debian package built for Linux Mint
22.x and Ubuntu 24.04:

```sh
sudo apt install ./dist/clipledge_0.1.0_amd64.deb
```

Installing the package adds the `clipledge` command, application-menu entry,
icon, AppStream metadata, and manual page. It recommends GNOME Keyring for the
desktop Secret Service used to store the database key.

To inspect the package checksum before installation:

```sh
sha256sum --check dist/SHA256SUMS
```

The development checksum is not cryptographically signed.

## First run

1. Open **ClipLedge** from the application menu, or run:

   ```sh
   clipledge --show
   ```

2. The Settings window opens on first use. Select **Unlock / Retry**. This creates
   a random encryption key in your desktop keyring and then creates the encrypted
   history database. If your keyring is locked, the desktop may ask you to unlock
   it.
3. Review the capture categories, application exclusions, retention values,
   appearance, status-icon option, and **Start at login** option.
4. Select **Save settings**.
5. Click **Shortcut binding**, press the complete shortcut you want, and select
   **Register shortcut (check for conflicts)**. Existing bindings are never
   replaced. If it conflicts, choose another binding or select **Configure
   Cinnamon shortcut…** and create one for this command:

   ```text
   clipledge --toggle
   ```

The application stays in the background after its window closes. If **Start at
login** is enabled, it starts future sessions with `clipledge --background`.

## Everyday use

Copy text, a link, an image, or files normally in another application. On a native
X11 session, eligible `CLIPBOARD` content is added to encrypted history. The
middle-click `PRIMARY` selection is not recorded.

Open the shelf with the configured shortcut, the application-menu entry, or the
status icon. Start typing to search, or choose one of these filters:

- **Everything** — all retained items
- **Pinned** — items excluded from count and age expiry
- **Text** — plain and rich text
- **Images** — locally generated bounded thumbnails
- **Links** — HTTP and HTTPS URLs; no page metadata is downloaded
- **Files** — file references; file contents are not copied into history

Select an item and press Enter. The shelf closes after restoring all supported
representations to the clipboard. Switch to the destination application and press
Ctrl+V. Historical file cuts are restored as copies to avoid moving old files.

Press Escape to close the shelf without changing the clipboard.

## Keyboard and pointer controls

While the search field has focus, normal text editing keys continue to edit the
query. Press Tab to enter the result cards.

| Control | Action |
| --- | --- |
| `Tab` | Move from search to the selected result |
| Arrow keys | Move between result cards and rows |
| `Enter` | Restore the selected item and hide the shelf |
| `Escape` | Hide the shelf without changing the clipboard |
| `Ctrl+F` | Return focus to search |
| `Ctrl+1` … `Ctrl+6` | Select Everything, Pinned, Text, Images, Links, or Files |
| `Alt+1` … `Alt+9` | Restore the corresponding visible result |
| `P` | Pin or unpin the focused result |
| `Delete` | Delete the focused result |
| `Space` | Preview the focused result |

`P`, Delete, and Space act on results only when a card has focus. You can also
double-click a card to restore it and use the pin and delete buttons on each card.
Deleting a pinned item asks for confirmation.

## Search and history management

Search covers previewable text, URLs, hostnames, filenames, and available source
labels. Queries are treated as literal word-prefix searches; search operators are
not executed, and arbitrary mid-word matches are not guaranteed. Results are
paged in groups of up to 60 cards.

The defaults retain 500 unpinned items for 30 days with a 500 MiB live-storage
target. Pinned items are exempt from count and age expiry. Under storage pressure,
unpinned items are removed first; if pinned data prevents admission, new capture
pauses and the shelf reports the problem.

**Clear history…** retains pinned items by default and asks for confirmation. In
Settings, **Reset encrypted history and key…** deletes every history item,
including pins, and removes the application's Secret Service key. Neither action
changes the current system clipboard.

## Pause, resume, and status

Use the pause button in the shelf, the status-icon menu, or these commands:

```sh
clipledge --pause
clipledge --resume
clipledge --status
```

Pausing stops new capture and does not queue a backlog. Screen lock, keyring
unavailability, an unsupported display backend, or storage pressure can also block
capture. `--resume` clears only the user pause; it cannot override those conditions.

The status command reports capabilities, running or paused state, counts,
versions, and non-sensitive error categories. When the application is not running,
it reports that state without opening the keyring or creating a database.

## Command-line reference

```text
clipledge [--show]       Show the shelf, starting the application if needed
clipledge --toggle       Show or hide the shelf
clipledge --hide         Hide the shelf
clipledge --pause        Pause new history capture
clipledge --resume       Resume user-paused capture when other gates permit it
clipledge --settings     Open Settings
clipledge --status       Print non-sensitive status
clipledge --background   Start without showing a window or prompting for a key
clipledge --quit         Exit the application
```

You can also read the installed manual page with `man clipledge`.

## Privacy and platform behaviour

History, thumbnails, metadata, and the search index are encrypted with system
SQLCipher. The 256-bit key is stored in Secret Service. There is no unencrypted
fallback. The application performs no telemetry, cloud sync, URL previews,
automatic update checks, or other intentional network requests.

Sensitive-content detection and source application exclusions are best effort.
The default exclusions include KeePassXC, KeePass 2, 1Password, and Bitwarden.
You can edit the comma-separated identities in Settings. A malicious process
running as the same logged-in user may still access an unlocked desktop session;
storage encryption does not change that operating-system trust boundary.

Native X11 supports background capture, source tracking, placement, and clipboard
preservation. On Wayland and XWayland, background capture is disabled. Existing
history can still be browsed and restored while the shelf has focus, subject to
the compositor's activation and placement rules.

Mint's packaged SQLCipher provides FTS4 rather than FTS5, so this build uses an
encrypted FTS4 search index. File-reference availability is shown conservatively
as unknown because checking a seemingly local path can trigger a network mount.

More detail is available in [Privacy](docs/privacy.md) and
[Architecture](docs/architecture.md).

## Troubleshooting

If the shelf says **History locked**, open Settings and select **Unlock / Retry**.
Make sure the desktop keyring is running and has a usable default collection. An
existing database is preserved if its key is missing or unavailable; `ClipLedge`
will not silently replace it.

If copied items do not appear, check:

- `clipledge --status` for paused, locked, backend, or storage states;
- that the session is X11 rather than Wayland or XWayland;
- that the source application is not in the exclusion list;
- that the item's capture category is enabled;
- that the payload is no larger than 25 MiB across retained representations; and
- that structured-secret detection did not reject the whole item.

If the status icon is missing, the desktop may not provide a StatusNotifierItem
watcher. The configured keyboard shortcut and `clipledge --show` still work.

Application data is stored under the standard XDG locations:

```text
~/.local/share/clipledge/history.db
~/.config/clipledge/settings.json
~/.config/autostart/org.clipledge.ClipLedge.desktop
```

The exact paths follow `XDG_DATA_HOME` and `XDG_CONFIG_HOME` when those variables
contain absolute paths.

## Uninstall

Quit the running application, then remove the package:

```sh
clipledge --quit
sudo apt remove clipledge
```

Package removal intentionally preserves encrypted history, preferences, and the
key. Use **Reset encrypted history and key…** before uninstalling if you want the
application to remove its own history and key.

## Build from source

Install the build dependencies on Linux Mint 22.x or Ubuntu 24.04:

```sh
sudo apt install cargo rustc libgtk-4-dev libsqlcipher-dev libssl-dev pkg-config
cargo fetch --locked
cargo build --release --locked --offline
./target/release/clipledge --show
```

The committed lockfile builds with Rust 1.75. To create a package for the current
host architecture:

```sh
sh scripts/package-deb.sh
```

ARM64 remains experimental until it is built and exercised on an ARM64 Mint or
Ubuntu system.

## Development and tests

```sh
cargo test --locked
xvfb-run -a cargo test --locked --test x11_backend -- --ignored --test-threads=1
xvfb-run -a dbus-run-session --config-file=tests/session.conf -- sh tests/smoke.sh
xvfb-run -a dbus-run-session --config-file=tests/session.conf -- sh tests/desktop-session.sh
```

The desktop tests require `xvfb`, `dbus-x11`, `xclip`, and `gnome-keyring`.
Optional screenshot capture also requires ImageMagick. The harnesses use synthetic
content, a disposable keyring, private XDG paths, and reject the normal display.
See [Verification](docs/verification.md) for local results and outstanding tests.

## Licence

`ClipLedge` is licensed under GPL-3.0-or-later. Author: Clive Bostock.
Third-party Rust crates retain their respective licences; packaged notices are
generated from the locked dependency graph.

## Acknowledgement

The horizontal, card-based shelf interaction was inspired by a macOS application
concept called Shelf demonstrated by Nathan B. Jones. ClipLedge is an independent
Linux implementation and is not affiliated with or endorsed by Nathan B. Jones.
