#!/bin/sh
# Author: Clive Bostock
# Date: 10-Sep-2026
# Purpose: Build a native development .deb without installing system files.
# Usage: sh scripts/package-deb.sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked --offline
package_stage=$(mktemp -d)
trap 'rm -rf "$package_stage"' EXIT
make install DESTDIR="$package_stage"
mkdir -p "$package_stage/DEBIAN" "$package_stage/usr/share/doc/clipledge" dist
install -m644 README.md LICENSE debian/copyright "$package_stage/usr/share/doc/clipledge/"
mkdir -p "$package_stage/usr/share/doc/clipledge/docs"
install -m644 docs/*.md "$package_stage/usr/share/doc/clipledge/docs/"
cargo metadata --format-version=1 --locked --offline > "$package_stage/metadata.json"
python3 scripts/collect-licenses.py --metadata "$package_stage/metadata.json" --output "$package_stage/usr/share/doc/clipledge/third-party-licenses"
rm "$package_stage/metadata.json"
architecture=$(dpkg --print-architecture)
dependency_line=$(dpkg-shlibdeps -O -e target/release/clipledge)
dependencies=${dependency_line#shlibs:Depends=}
dependencies=$(printf '%s' "$dependencies" | sed 's/libsqlcipher1 ([^)]*)/libsqlcipher1 (>= 4.5.6)/')
cat > "$package_stage/DEBIAN/control" <<CONTROL
Package: clipledge
Version: 0.1.0
Architecture: $architecture
Section: utils
Priority: optional
Maintainer: Clive Bostock <clive@localhost>
Depends: $dependencies
Recommends: gnome-keyring
Conflicts: mint-shelf (<= 0.1.0)
Replaces: mint-shelf (<= 0.1.0)
Description: private native clipboard history for Cinnamon
 Development build with SQLCipher encryption and a GTK 4 shelf.
 Native X11 supports capture; Wayland supports browsing and restoration.
CONTROL
gzip -n "$package_stage/usr/share/man/man1/clipledge.1"
find "$package_stage" -type d -exec chmod 0755 {} +
artifact="dist/clipledge_0.1.0_${architecture}.deb"
dpkg-deb --root-owner-group --build "$package_stage" "$artifact"
(cd dist && sha256sum "clipledge_0.1.0_${architecture}.deb" > SHA256SUMS)
printf 'Built %s (development build; checksum is not a signed release).\n' "$artifact"
