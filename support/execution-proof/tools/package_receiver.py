#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Freeze already-built receiver/helper binaries into a macOS qualification app."""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import re
import subprocess
import sys


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin-dir", required=True, type=Path)
    parser.add_argument("--bundle", required=True, type=Path)
    parser.add_argument("receiver_args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("macOS packaging requires APFS clone copies")
    bundle = args.bundle.absolute()
    if bundle.suffix != ".app" or bundle.exists():
        parser.error("choose a fresh .app path; existing evidence is never overwritten")
    receiver_args = args.receiver_args
    if receiver_args[:1] == ["--"]:
        receiver_args = receiver_args[1:]
    if "--helper" in receiver_args or "--output" not in receiver_args:
        parser.error("supply --output; the bundle supplies its frozen helper")
    try:
        output = Path(receiver_args[receiver_args.index("--output") + 1])
    except IndexError:
        parser.error("--output needs an absolute path")
    if not output.is_absolute() or output.exists():
        parser.error("choose a fresh absolute output directory")
    sources = {name: args.bin_dir / name for name in
               ["cue-window-receiver", "cue-execution-proof"]}
    for source in sources.values():
        if not source.is_file():
            parser.error(f"missing executable: {source}")
    binary_dir = bundle / "Contents/MacOS"
    resources = bundle / "Contents/Resources"
    binary_dir.mkdir(parents=True)
    resources.mkdir()
    hashes = {}
    for name, source in sources.items():
        destination = binary_dir / name
        subprocess.run(["cp", "-c", "-p", str(source), str(destination)], check=True)
        hashes[name] = digest(source)
        if digest(destination) != hashes[name]:
            raise RuntimeError(f"frozen executable hash mismatch: {name}")
    name = re.sub(r"[^A-Za-z0-9.-]", "-", bundle.stem)
    info = {"CFBundleIdentifier": f"made.merely.cue.{name}",
            "CFBundleName": "CueReceiver", "CFBundleDisplayName": "CueReceiver",
            "CFBundleExecutable": "cue-window-receiver", "CFBundlePackageType": "APPL",
            "NSHighResolutionCapable": True, "NSPrincipalClass": "NSApplication"}
    with (bundle / "Contents/Info.plist").open("wb") as target:
        plistlib.dump(info, target)
    receiver_args += ["--helper", str(binary_dir / "cue-execution-proof")]
    (resources / "arguments.json").write_text(json.dumps(receiver_args) + "\n")
    (resources / "binary-hashes.json").write_text(json.dumps(hashes, indent=2) + "\n")
    print(bundle)


if __name__ == "__main__":
    main()
