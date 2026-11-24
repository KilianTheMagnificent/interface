#!/usr/bin/env python3
"""Stage only the two relay crates and public build configuration; never the workspace."""
import argparse
import pathlib
import re
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("destination", type=pathlib.Path)
arguments = parser.parse_args()
source = pathlib.Path(__file__).resolve().parents[2]
destination = arguments.destination.resolve()
if destination.exists():
    raise SystemExit("Choose a new empty destination; existing files are never overwritten.")
destination.mkdir(parents=True)
manifest = (source / "Cargo.toml").read_text()
manifest, replacements = re.subn(r"members = \[.*?\]", 'members = ["crates/interface-contracts", "crates/interface-remote"]', manifest, count=1, flags=re.S)
assert replacements == 1
(destination / "Cargo.toml").write_text(manifest)
shutil.copyfile(source / "Cargo.lock", destination / "Cargo.lock")
for crate in ["interface-contracts", "interface-remote"]:
    crate_source = source / "crates" / crate
    crate_destination = destination / "crates" / crate
    crate_destination.mkdir(parents=True)
    shutil.copyfile(crate_source / "Cargo.toml", crate_destination / "Cargo.toml")
    for file in (crate_source / "src").rglob("*.rs"):
        if file.is_symlink():
            raise SystemExit("Source symlinks are not included in a relay build context.")
        target = crate_destination / file.relative_to(crate_source)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(file, target)
for name in ["Dockerfile", "fly.example.toml"]:
    shutil.copyfile(source / "deploy/phone-relay" / name, destination / name)
(destination / ".dockerignore").write_text("target\n.git\n*.log\n")
# Cargo removes unused workspace packages while retaining the already resolved
# versions needed by these crates. Offline resolution cannot download new versions.
subprocess.run(["cargo", "metadata", "--offline", "--format-version", "1"], cwd=destination, check=True, stdout=subprocess.DEVNULL)
print(f"Prepared minimal relay build context: {destination}")
