"""Collect Cargo dependency licences for binary redistribution."""
# Author: Clive Bostock
# Date: 11-Sep-2026
# Description: Copy dependency notices into the staged Debian documentation.
# Purpose: Preserve third-party licence texts in binary packages.
# Usage: python3 scripts/collect-licenses.py --metadata metadata.json --output licenses

import argparse
import json
import shutil
from pathlib import Path


def collect(metadata: Path, output: Path) -> None:
    """Collect licence files and an index from Cargo's resolved metadata.

    Args:
        metadata: Cargo metadata JSON file produced with the locked graph.
        output: Destination directory inside the staged package.
    """
    packages = json.loads(metadata.read_text(encoding="utf-8"))["packages"]
    output.mkdir(parents=True, exist_ok=True)
    lines = ["Third-party Cargo dependency licences", ""]
    for package in sorted(packages, key=lambda item: (item["name"], item["version"])):
        if package["name"] == "clipledge":
            continue
        root = Path(package["manifest_path"]).resolve().parent
        destination = output / f'{package["name"]}-{package["version"]}'
        destination.mkdir(exist_ok=True)
        found = []
        for pattern in ("LICENSE*", "COPYING*", "COPYRIGHT*", "UNLICENSE*", "licenses/*"):
            for source in root.glob(pattern):
                if source.is_file() and source.resolve().is_relative_to(root):
                    shutil.copyfile(source, destination / source.name)
                    found.append(source.name)
        lines.append(
            f'{package["name"]} {package["version"]}: {package.get("license") or "see source"}; '
            f'notices: {", ".join(sorted(set(found))) or "see crate source distribution"}'
        )
    (output / "INDEX.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> None:
    """Parse packaging paths and collect licence notices."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    collect(args.metadata, args.output)


if __name__ == "__main__":
    main()
