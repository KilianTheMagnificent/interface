"""The public copy holds only the listed paths and nothing personal.

    python3 -m unittest discover -s scripts -p 'test_*.py'
"""
from __future__ import annotations
import subprocess
import tempfile
import unittest
from pathlib import Path

import export_public


def git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com", *args],
                   cwd=cwd, check=True, capture_output=True)


class PublicExportTests(unittest.TestCase):
    def make_source(self, directory: Path) -> Path:
        source = directory / "private"
        (source / "crates").mkdir(parents=True)
        (source / "docs/archive").mkdir(parents=True)
        (source / "crates/lib.rs").write_text("pub fn greet() {}\n")
        (source / "README.md").write_text("# App\n")
        (source / "docs/CODEMAP.md").write_text("map\n")
        (source / "docs/TODO.md").write_text("the owner's list\n")
        (source / "docs/archive/old.md").write_text("retired\n")
        (source / "docs/archive/feature").mkdir()
        (source / "docs/archive/feature/README.md").write_text("what it was\n")
        (source / "docs/archive/feature/handoff.md").write_text("the owner's notes\n")
        (source / "RESEARCH.md").write_text("notes on another product\n")
        git(source, "init", "-q")
        git(source, "add", "-A")
        git(source, "commit", "-q", "-m", "first")
        return source

    def test_private_paths_and_unlisted_top_level_files_stay_out(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            source = self.make_source(d)
            public = d / "public"
            (public / ".git").mkdir(parents=True)
            (public / "stale.rs").write_text("removed upstream")

            count = export_public.export(source, public, ["crates", "docs", "README.md"],
                                         ["docs/TODO.md", "docs/archive"], ["docs/archive/*/README.md"])

            self.assertEqual(count, 4)
            self.assertEqual(sorted(p.relative_to(public).as_posix() for p in (public / "docs/archive").rglob("*")),
                             ["docs/archive/feature", "docs/archive/feature/README.md"])
            self.assertTrue((public / "crates/lib.rs").is_file())
            self.assertTrue((public / "docs/CODEMAP.md").is_file())
            self.assertFalse((public / "docs/TODO.md").exists())
            self.assertFalse((public / "RESEARCH.md").exists())
            self.assertFalse((public / "stale.rs").exists())
            self.assertTrue((public / ".git").is_dir())

    def test_export_is_made_from_committed_files_only(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            source = self.make_source(d)
            (source / "crates/lib.rs").write_text("pub fn not_committed() {}\n")
            with self.assertRaisesRegex(ValueError, "commit these first"):
                export_public.export(source, d / "public", ["crates", "README.md"], [])

    def test_personal_addresses_home_paths_and_dead_front_links_stop_it(self):
        with tempfile.TemporaryDirectory() as directory:
            d = Path(directory)
            (d / "docs").mkdir()
            (d / "a.rs").write_text("let who = \"real.person@" + "fastmail.com\";\n"
                                    "let test = \"owner@example.com\";\n"
                                    "let sample = \"jane.doe+work@gmail.com\";\n")
            (d / "b.ts").write_text("const home = \"/Users/" + "andreromero/project\";\n"
                                    "const other = \"/Users/example/project\";\n")
            (d / "README.md").write_text("[list](docs/TODO.md) [map](docs/CODEMAP.md)\n")
            (d / "docs/CODEMAP.md").write_text("[list](TODO.md)\n")
            findings, notes = export_public.private_findings(d)
            self.assertEqual(len(findings), 3, findings)
            self.assertTrue(any("a.rs:1: email address" in f for f in findings))
            self.assertTrue(any("b.ts:1: the owner's home folder path" in f for f in findings))
            self.assertTrue(any("README.md:1: link to a file that is not published: docs/TODO.md" in f
                                for f in findings))
            self.assertEqual(len(notes), 1, notes)

    def test_a_key_shaped_string_outside_known_test_data_is_not_waved_through(self):
        flagged = ["crates/provider-replay/fixtures/claude/simple.json", "electron/src/fixtures/threads-v34.json",
                   "electron/src/main/main.ts"]
        self.assertEqual(export_public.unknown_secret_files(flagged), ["electron/src/main/main.ts"])


if __name__ == "__main__":
    unittest.main()
