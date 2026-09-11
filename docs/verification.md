# Verification

Run from the repository root with the committed Cargo.lock:

```sh
cargo fmt --check
cargo test --locked
xvfb-run -a cargo test --locked --test x11_backend -- --ignored --test-threads=1
xvfb-run -a dbus-run-session --config-file=tests/session.conf -- sh tests/smoke.sh
xvfb-run -a dbus-run-session --config-file=tests/session.conf -- sh tests/desktop-session.sh
cargo build --release --locked
sh scripts/package-deb.sh
dpkg-deb --info dist/clipledge_0.1.0_amd64.deb
```

Integration tests are ignored by an ordinary cargo test run because clipboard
access must be isolated. The shell harnesses refuse DISPLAY=:0 and allocate
private XDG paths. The desktop-session harness starts a disposable test keyring.
Optional screenshot output is enabled with CLIPLEDGE_TEST_SCREENSHOT set to an
absolute output path; only synthetic fixture content is rendered.

The 10,000-entry search test uses a real encrypted database and asserts p95 below
100 ms across twenty queries. The indexed implementation measured approximately
4 ms p95 locally. This is a search benchmark, not a whole-UI latency measurement.

SQLCipher tests check fixture absence from disk artefacts, key rejection, logical
round trips, FTS cleanup, and cancellation before commit. Absence of a fixture
string alone is not used as proof of encryption: cipher availability and keyed
integrity checks are also required.

CI runs the Rust 1.75 build, unit tests, Xvfb protocol tests, and private desktop
checks. ARM64 and real Cinnamon/Wayland acceptance require additional runners or
manual testing as listed in compatibility.md.

## Local validation, 11 September 2026

The 13 core tests, two isolated X11 tests, disposable Secret Service/GTK test,
and release-binary lifecycle smoke test passed on the local amd64 environment.
The rendered shelf was inspected using synthetic history. These checks do not
establish the real Cinnamon acceptance criteria.

The isolated release smoke test measured 133,488 KiB idle RSS (about 130 MiB)
under Xvfb with software rendering. This exceeds the initial 75 MiB target.
Real-desktop memory and shortcut latency measurements remain required; the
development package must not be presented as meeting those performance targets.
