#!/usr/bin/env python3
"""Generate Tauri release overrides while keeping Apple's versions numeric."""
import argparse
import json
from pathlib import Path
import plistlib
import re

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("version", help="SemVer, optionally prefixed with v")
parser.add_argument("output", type=Path, help="Output JSON path, with a sibling generated plist")
parser.add_argument("--signing-identity", help="Omit for the configured local ad-hoc identity")
args = parser.parse_args()
version = args.version.removeprefix("v")
match = re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version)
if not match:
    parser.error("version must be a semantic version such as 0.11.0-beta.1")
base = ".".join(match.groups())
root = Path(__file__).resolve().parent.parent
source = root / "apps/desktop/src-tauri/Info.plist"
with source.open("rb") as file:
    info = plistlib.load(file)
# Keep the tag in Tauri's app version and artifact names. Apple bundle fields
# use numeric components, including for a directly distributed prerelease.
info["CFBundleShortVersionString"] = base
info["CFBundleVersion"] = base
output = args.output.resolve()
output.parent.mkdir(parents=True, exist_ok=True)
plist_path = output.with_suffix(".plist")
with plist_path.open("wb") as file:
    plistlib.dump(info, file)
macos = {"infoPlist": str(plist_path)}
if args.signing_identity:
    macos["signingIdentity"] = args.signing_identity
output.write_text(json.dumps({"version": version, "bundle": {"macOS": macos}}, indent=2) + "\n")
print(output)
