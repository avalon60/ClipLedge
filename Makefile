# Author: Clive Bostock
# Date: 10-Sep-2026
# Purpose: Build, test, and stage ClipLedge.
# Usage: make test; make install DESTDIR=/tmp/clipledge-package
PREFIX ?= /usr
.PHONY: build test install
build:
	cargo build --release --locked
test:
	cargo test --locked
install: build
	install -Dm755 target/release/clipledge $(DESTDIR)$(PREFIX)/bin/clipledge
	install -Dm644 data/org.clipledge.ClipLedge.desktop $(DESTDIR)$(PREFIX)/share/applications/org.clipledge.ClipLedge.desktop
	install -Dm644 data/org.clipledge.ClipLedge.metainfo.xml $(DESTDIR)$(PREFIX)/share/metainfo/org.clipledge.ClipLedge.metainfo.xml
	install -Dm644 data/icons/hicolor/scalable/apps/org.clipledge.ClipLedge.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/org.clipledge.ClipLedge.svg
	install -Dm644 data/clipledge.1 $(DESTDIR)$(PREFIX)/share/man/man1/clipledge.1
