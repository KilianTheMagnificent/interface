#!/usr/bin/env python3
"""Write the public copy of the source: the listed paths, as committed, checked for private data.

    python3 scripts/export_public.py "<checkout of the public repository>"

The public repository is a separate one with its own history. This repository's history, its
to-do and resume lists, archived notes and research on other products are never published.
The script replaces the checkout's files, keeps its .git, and stops if anything private
would go out. It does not commit or push.
"""
from __future__ import annotations
import fnmatch
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# A new top-level path stays private until it is added here, after reading it.
PUBLIC_PATHS = [
    "apps", "crates", "electron", "xtask", "deploy", "scripts", "skills", "docs",
    ".cargo", ".github", ".gitignore", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
    "AGENTS.md", "CLAUDE.md", "README.md", "LICENSE", "CONTRIBUTING.md", "SECURITY.md",
    "CODE_OF_CONDUCT.md", "install.sh",
]
# Inside the public paths: the owner's to-do and resume lists, retired-feature notes and
# research on other products.
PRIVATE_PATHS = ["docs/TODO.md", "docs/TODO-done.md", "docs/archive", "docs/research"]
# Kept inside a private folder: the one-page record of each retired feature, which the
# code map (`docs/agent-map.json`, checked by the gate) and the public docs point at.
KEPT_IN_PRIVATE = ["docs/archive/*/README.md"]
# Files a stranger reads first: a link from one of these to an unpublished file stops the export.
FRONT_FILES = {"README.md", "CONTRIBUTING.md", "SECURITY.md", "CODE_OF_CONDUCT.md"}

EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@((?:[A-Za-z0-9-]+\.)+[A-Za-z]{2,})\b")
# Placeholder and vendor addresses in tests and fixtures; anything else is a person's.
PUBLIC_EMAIL_DOMAINS = {
    "example.com", "example.org", "example.test", "example.invalid", "users.noreply.github.com",
    "anthropic.com", "github.com", "gmail.com", "izs.me", "x.com", "b.com", "b.test", "else.test",
    "interface.invalid", "2x.png", "3x.png",
}
PLACEHOLDER_MAILBOXES = {"jane.doe+work@gmail.com", "a.person@gmail.com"}
# Written in two pieces so this file does not contain the path it looks for.
PRIVATE_HOME = re.compile("/Users/" + r"andreromero\b")
MARKDOWN_LINK = re.compile(r"\]\(([^)#\s<]+)(?:#[^)]*)?\)")


def is_public_email(address: str, domain: str) -> bool:
    domain = domain.lower()
    if domain == "gmail.com":
        return address.lower() in PLACEHOLDER_MAILBOXES
    return domain in PUBLIC_EMAIL_DOMAINS or any(domain.endswith("." + known) for known in PUBLIC_EMAIL_DOMAINS)


def private_findings(root: Path) -> tuple[list[str], list[str]]:
    """What stops the export (personal addresses, the owner's home folder, dead links in the
    front files) and what is only reported (links from deeper docs to unpublished files)."""
    findings, notes = [], []
    for path in sorted(p for p in root.rglob("*") if p.is_file() and ".git" not in p.relative_to(root).parts):
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue  # binary assets
        name = path.relative_to(root)
        for number, line in enumerate(text.splitlines(), 1):
            for match in EMAIL.finditer(line):
                if not is_public_email(match.group(0), match.group(1)):
                    findings.append(f"{name}:{number}: email address {match.group(0)}")
            if PRIVATE_HOME.search(line):
                findings.append(f"{name}:{number}: the owner's home folder path")
            if path.suffix == ".md":
                for match in MARKDOWN_LINK.finditer(line):
                    target = match.group(1)
                    if "://" in target or target.startswith("mailto:") or (path.parent / target).exists():
                        continue
                    message = f"{name}:{number}: link to a file that is not published: {target}"
                    (findings if str(name) in FRONT_FILES else notes).append(message)
    return findings, notes


def uncommitted(root: Path, paths: list[str]) -> list[str]:
    out = subprocess.run(["git", "status", "--porcelain", "--", *paths],
                         cwd=root, capture_output=True, text=True, check=True).stdout
    return [line for line in out.splitlines() if line.strip()]


def export(root: Path, destination: Path, paths: list[str], private: list[str], keep: list[str] = ()) -> int:
    """Replace `destination`'s files (not its .git) with `paths` minus `private` (apart from
    the `keep` patterns inside it), as committed at HEAD."""
    root, destination = root.resolve(), destination.resolve()
    if destination == root or root in destination.parents or destination in root.parents:
        raise ValueError("the public checkout must be outside this repository")
    if destination.exists() and any(destination.iterdir()) and not (destination / ".git").exists():
        raise ValueError(f"{destination} is not empty and is not a git checkout; refusing to replace its files")
    dirty = uncommitted(root, paths)
    if dirty:
        raise ValueError("commit these first, the public copy is made from HEAD:\n  " + "\n  ".join(dirty))
    destination.mkdir(parents=True, exist_ok=True)
    for child in destination.iterdir():
        if child.name == ".git":
            continue
        shutil.rmtree(child) if child.is_dir() and not child.is_symlink() else child.unlink()
    with tempfile.NamedTemporaryFile(suffix=".tar") as archive:
        subprocess.run(["git", "archive", "--format=tar", "-o", archive.name, "HEAD", "--", *paths],
                       cwd=root, check=True)
        subprocess.run(["/usr/bin/tar", "-xf", archive.name, "-C", str(destination)], check=True)
    for relative in private:
        target = destination / relative
        if not target.is_dir() or target.is_symlink():
            target.unlink(missing_ok=True)
            continue
        for path in sorted(target.rglob("*"), reverse=True):
            name = path.relative_to(destination).as_posix()
            if path.is_dir() and not path.is_symlink():
                if not any(path.iterdir()):
                    path.rmdir()
            elif not any(fnmatch.fnmatchcase(name, kept) for kept in keep):
                path.unlink()
        if not any(target.iterdir()):
            target.rmdir()
    return sum(1 for p in destination.rglob("*") if p.is_file() and ".git" not in p.relative_to(destination).parts)


# Key-shaped strings gitleaks flags that were each read and are test data: sample tokens
# in tests, message ids in recorded provider sessions, public keys in protocol vectors.
KNOWN_TEST_DATA = (
    "crates/provider-replay/fixtures/", "electron/src/fixtures/",
    "apps/interface-host/src/instance_lock.rs", "apps/interface-host/src/provider_capture/native.rs",
    "crates/interface-review/src/thread_ship_tests.rs", "crates/interface-review/src/diagnostics.rs",
    "crates/provider-replay/src/scrub.rs", "crates/interface-terminal/src/launch_tests.rs",
    "docs/PROTOCOL_FACTS.md", "electron/src/renderer/components/ThreadComposer.test.ts",
    "electron/src/shared/visual-frame.test.ts",
)


def unknown_secret_files(flagged: list[str]) -> list[str]:
    """The flagged files that are not known test data."""
    return sorted({name for name in flagged if not name.startswith(KNOWN_TEST_DATA)})


def scan_secrets(destination: Path) -> None:
    gitleaks = shutil.which("gitleaks")
    if not gitleaks:
        raise ValueError("gitleaks is not installed (brew install gitleaks); the public copy must be scanned")
    with tempfile.NamedTemporaryFile(suffix=".json") as report:
        subprocess.run([gitleaks, "dir", str(destination), "--no-banner", "--redact",
                        "--report-format", "json", "--report-path", report.name],
                       capture_output=True, text=True)
        findings = json.loads(Path(report.name).read_text() or "[]")
    prefix = str(destination.resolve()) + "/"
    flagged = [str(Path(finding["File"]).resolve()).removeprefix(prefix) for finding in findings]
    unknown = unknown_secret_files(flagged)
    if unknown:
        raise ValueError("gitleaks found key-shaped strings outside the known test data. Read each; remove a real "
                         "secret (and rotate it), or add a test file to KNOWN_TEST_DATA:\n  " + "\n  ".join(unknown))


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    destination = Path(sys.argv[1]).expanduser()
    try:
        count = export(ROOT, destination, PUBLIC_PATHS, PRIVATE_PATHS, KEPT_IN_PRIVATE)
        findings, notes = private_findings(destination)
        if findings:
            raise ValueError("private data in the public copy:\n  " + "\n  ".join(findings))
        scan_secrets(destination)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        sys.exit(f"Public export stopped: {error}")
    head = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                          capture_output=True, text=True, check=True).stdout.strip()
    print(f"Public copy written to {destination}: {count} files from {head}; no personal address, home path or "
          "secret found.")
    if notes:
        print(f"{len(notes)} links in deeper docs point at files that are not published:\n  " + "\n  ".join(notes))
    print("Not committed or pushed. Review `git status` there, then commit.")


if __name__ == "__main__":
    main()
