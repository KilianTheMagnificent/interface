# Security Policy

## Scope

Interface is a macOS app that runs AI coding agents on your Mac. It:

- starts agent command-line tools (Claude Code, Codex and others) as you, in the folders you
  open, and passes their requests for approval to you;
- runs terminals, reads and edits project files, and runs `git` and `gh` for the threads
  you use them in;
- shows web pages in its Browser pane, with no access to the app or your files;
- keeps its data in your user account only, and holds no provider credentials: each agent
  tool signs in by itself;
- optionally accepts a paired iPhone over an encrypted connection (off until you turn it on);
- updates itself from GitHub Releases, installing only a release signed with the
  maintainer's key and by the same Apple developer team as the running app.

It has no accounts or server of its own.

Out of scope: what an agent does with permission you gave it (a thread in Bypass mode, a
command you approved), and anything that needs another program already running as you on
the Mac with your files.

## Reporting a Vulnerability

Please report a security issue privately through GitHub's
[Private Vulnerability Reporting](https://github.com/KilianTheMagnificent/interface/security/advisories/new)
instead of opening a public issue. Examples: a project folder or web page that makes
Interface run something without your approval, an agent reaching another thread's
permissions, an update that installs without valid signatures, a way to pair a phone
without approving it on the Mac.

Please include the Interface version, your macOS version and steps to reproduce, without
real prompts, project files or keys.
