//! Pure approval policy for unattended agent teammates.
//!
//! Every tool call an agent makes arrives here as provider-neutral
//! [`ApprovalFacts`]. [`classify`] either allows it or sends it to the owner.
//! It never fails open: anything it cannot resolve is an owner question.
// ponytail: risky-list shell model; scripts written in scope can still send data. Allowlist mode if decision logs show misses.
// A program's identity is resolved through symlinks and against binaries the
// same command created; a binary copied or linked in one command and run in a
// later command is within that script ceiling.

use std::path::PathBuf;

use interface_contracts::agents::ApprovalFacts;

mod paths;
mod risk;
mod shell;
mod targets;

#[cfg(test)]
use paths::wildcard_matches;
use paths::{Context, Effect, expand_home};
use shell::{DIRECTORY_CHANGES, FILE_COMMANDS, shell, shell_checked};
use targets::read_operand_options;

/// Where an agent may write: its folders, its working directory and the
/// owner's home (for `~` expansion and the `rm -r ~` guard).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentScope {
    pub roots: Vec<PathBuf>,
    pub cwd: PathBuf,
    pub home_dir: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyDecision {
    Allow { reason: String },
    AskOwner { reason: String },
}

impl PolicyDecision {
    #[must_use]
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    #[must_use]
    pub fn reason(&self) -> &str {
        match self {
            Self::Allow { reason } | Self::AskOwner { reason } => reason,
        }
    }
}

const CREDENTIAL: &str = "credentialPath";
const OUTSIDE: &str = "outsideScope";
const UNRESOLVED: &str = "unresolvedPath";
const UNPARSEABLE: &str = "unparseable";
const RM_ROOT: &str = "risky:rm-root";
const INTERPRETER: &str = "risky:interpreter";
const AGENT_CONFIG: &str = "risky:agent-config";
const CWD_OUTSIDE: &str = "cwdOutsideScope";

/// Directories holding an agent harness's own settings, hooks and MCP servers.
const AGENT_CONFIG_DIRECTORIES: &[&str] = &[".claude", ".codex", ".grok"];

/// Symlink hops one path may take before it is treated as unresolvable.
const LINK_BUDGET: usize = 40;
/// Candidate working directories a chain of `cd`s may fan out to.
const MAX_WORKING_DIRECTORIES: usize = 32;

type Verdict = Result<(), &'static str>;

#[must_use]
pub fn classify(facts: &ApprovalFacts, scope: &AgentScope) -> PolicyDecision {
    use ApprovalFacts as F;
    let anchored = scope.cwd.is_absolute() && scope.home_dir.is_absolute();
    match facts {
        F::Read { .. }
        | F::Search { .. }
        | F::Write { .. }
        | F::Delete { .. }
        | F::Shell { .. }
            if !anchored =>
        {
            ask(UNRESOLVED)
        }
        F::Read { paths } => reads(paths, scope, "read"),
        F::Search { paths } => reads(paths, scope, "search"),
        F::Write { paths } => writes(paths, scope, Effect::Write),
        F::Delete { paths } => writes(paths, scope, Effect::RemoveTree),
        // A search query can carry what an agent has read to the search
        // provider, so searching asks unless the owner switched it on for this
        // agent (owner's choice, 2026-11-21; see `classify_task`).
        F::WebSearch {} => ask("webSearch"),
        // A fetched URL can carry anything an agent has read off the Mac, so
        // every fetch is the owner's call (owner's choice, 2026-10-14). The
        // reason still says whether the method changes anything remotely.
        F::WebFetch { method, .. } if method.eq_ignore_ascii_case("GET") => ask("webFetch"),
        F::WebFetch { .. } => ask("webMethod"),
        F::Mcp { .. } => ask("mcpAction"),
        F::Internal { .. } => allow("internal"),
        F::Unknown { .. } => ask("unknownTool"),
        F::Shell { command, cwd } => shell(command, cwd.as_deref(), scope),
    }
}

/// Owner switches that loosen [`classify`] for one agent. Each one opens only
/// its own kind of call; every other decision is unchanged.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OwnerSwitches {
    /// The legacy risky-list ceiling for opaque shell commands.
    pub allow_opaque: bool,
    /// Web searches without asking. Web fetches still ask.
    pub allow_web_search: bool,
}

/// An agent's policy with its owner's switches: [`classify`], loosened only
/// where a switch is on. Searching the web without asking opens web searches
/// and nothing else.
#[must_use]
pub fn classify_for_owner(
    facts: &ApprovalFacts,
    scope: &AgentScope,
    switches: OwnerSwitches,
) -> PolicyDecision {
    if switches.allow_web_search && matches!(facts, ApprovalFacts::WebSearch {}) {
        return allow("webSearchAllowed");
    }
    classify_task(facts, scope, switches.allow_opaque)
}

/// Task policy adds an explicit owner opt-in for the legacy opaque-command ceiling.
#[must_use]
pub fn classify_task(
    facts: &ApprovalFacts,
    scope: &AgentScope,
    allow_opaque: bool,
) -> PolicyDecision {
    let decision = classify(facts, scope);
    if allow_opaque || !decision.is_allow() {
        return decision;
    }
    if let ApprovalFacts::Shell { command, cwd } = facts {
        return shell_checked(command, cwd.as_deref(), scope, true);
    }
    decision
}

/// Programs a strict task may run whose operands are never read as file
/// contents, so they need no read-operand scope check. A program that prints
/// or summarizes the files it names belongs in `READ_OPERAND_PROGRAMS`.
const UNCHECKED_OPERAND_PROGRAMS: &[&str] = &[
    "pwd", "df", "printf", "echo", "test", "[", "[[", "true", "false", "sleep",
];

/// Programs a strict task may run: file commands whose targets the walk
/// checks, directory changes, reading programs whose operands it checks, and
/// `UNCHECKED_OPERAND_PROGRAMS`.
fn known_command_effects(program: &str) -> bool {
    (FILE_COMMANDS.contains(&program)
        && !matches!(program, "sed" | "gsed" | "rsync" | "ditto" | "install"))
        || DIRECTORY_CHANGES.contains(&program)
        || read_operand_options(program).is_some()
        || UNCHECKED_OPERAND_PROGRAMS.contains(&program)
}

fn allow(reason: &str) -> PolicyDecision {
    PolicyDecision::Allow {
        reason: reason.to_owned(),
    }
}

fn ask(reason: &str) -> PolicyDecision {
    PolicyDecision::AskOwner {
        reason: reason.to_owned(),
    }
}

/// Credential material is reported before scope, across every path. An
/// automatic read stays inside the granted folders: web fetches ask, but an
/// owner may let searches through, so anything read can still leave the Mac.
fn reads(paths: &[String], scope: &AgentScope, reason: &str) -> PolicyDecision {
    let context = Context::new(scope);
    let joined: Vec<PathBuf> = paths
        .iter()
        .map(|path| scope.cwd.join(expand_home(path, &scope.home_dir)))
        .collect();
    for path in &joined {
        match context.credential_at(path) {
            Some(false) => {}
            Some(true) => return ask(CREDENTIAL),
            None => return ask(UNRESOLVED),
        }
    }
    if !joined.iter().all(|path| context.in_scope(path)) {
        return ask(OUTSIDE);
    }
    allow(reason)
}

fn writes(paths: &[String], scope: &AgentScope, effect: Effect) -> PolicyDecision {
    if paths.is_empty() {
        return ask(UNRESOLVED);
    }
    let context = Context::new(scope);
    for path in paths {
        if path.is_empty() {
            return ask(UNRESOLVED);
        }
        let joined = scope.cwd.join(expand_home(path, &scope.home_dir));
        if let Err(reason) = context.check_path(&joined, effect, &[]) {
            return ask(reason);
        }
    }
    allow("inScopeWrite")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    use super::{
        AgentScope, ApprovalFacts, OwnerSwitches, PolicyDecision, classify, classify_for_owner,
        classify_task,
    };

    fn scope() -> AgentScope {
        AgentScope {
            roots: vec![
                PathBuf::from("/work/agents/Bot"),
                PathBuf::from("/work/project"),
            ],
            cwd: PathBuf::from("/work/agents/Bot"),
            home_dir: PathBuf::from("/Users/me"),
        }
    }

    #[test]
    fn task_scripts_need_explicit_owner_opt_in_even_when_the_legacy_policy_allows_them() {
        let scope = scope();
        for command in [
            "python3 -c 'from pathlib import Path; Path(\"/outside/revoked.txt\").write_text(\"x\")'",
            "node script.js",
            "bash script.sh",
            "./unknown-program",
            "npm test",
            "sort -o /outside/result input",
            "uniq input /outside/result",
            "file -C -m local-magic",
            "git diff --output=/outside/result",
            "sed 'w /outside/result' input",
        ] {
            let facts = ApprovalFacts::Shell {
                command: command.into(),
                cwd: None,
            };
            assert!(
                !classify_task(&facts, &scope, false).is_allow(),
                "{command}"
            );
            assert_eq!(
                classify_task(&facts, &scope, true),
                classify(&facts, &scope)
            );
        }
        let read = ApprovalFacts::Shell {
            command: "ls -la".into(),
            cwd: None,
        };
        assert!(classify_task(&read, &scope, false).is_allow());
    }

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn read(values: &[&str]) -> ApprovalFacts {
        ApprovalFacts::Read {
            paths: paths(values),
        }
    }

    fn search(values: &[&str]) -> ApprovalFacts {
        ApprovalFacts::Search {
            paths: paths(values),
        }
    }

    fn write(values: &[&str]) -> ApprovalFacts {
        ApprovalFacts::Write {
            paths: paths(values),
        }
    }

    fn delete(values: &[&str]) -> ApprovalFacts {
        ApprovalFacts::Delete {
            paths: paths(values),
        }
    }

    fn fetch(method: &str, url: &str) -> ApprovalFacts {
        ApprovalFacts::WebFetch {
            url: url.to_owned(),
            method: method.to_owned(),
        }
    }

    fn mcp(server: &str, tool: &str) -> ApprovalFacts {
        ApprovalFacts::Mcp {
            server: server.to_owned(),
            tool: tool.to_owned(),
        }
    }

    fn shell(command: &str) -> ApprovalFacts {
        ApprovalFacts::Shell {
            command: command.to_owned(),
            cwd: None,
        }
    }

    fn shell_in(command: &str, cwd: &str) -> ApprovalFacts {
        ApprovalFacts::Shell {
            command: command.to_owned(),
            cwd: Some(cwd.to_owned()),
        }
    }

    const ALLOW: bool = true;
    const ASK: bool = false;

    /// `(command, expect_allow, reason)` with no `cwd` override.
    const SHELL_TABLE: &[(&str, bool, &str)] = &[
        // The task brief's minimum rows.
        ("ls -la", ALLOW, "shellInScope"),
        ("cargo test -p foo", ALLOW, "shellInScope"),
        (
            "cd /work/project && npm test | tail -5",
            ALLOW,
            "shellInScope",
        ),
        ("git status && git diff", ALLOW, "shellInScope"),
        ("git commit -am wip", ALLOW, "shellInScope"),
        ("git push origin main", ASK, "risky:git-push"),
        ("git -C /work/project push", ASK, "risky:git-push"),
        ("git reset --hard HEAD~1", ASK, "risky:git-reset-hard"),
        ("git clean -fdx", ASK, "risky:git-clean"),
        ("git branch -D feature", ASK, "risky:git-branch-delete"),
        ("gh pr create --fill", ASK, "risky:gh"),
        ("gh pr view 12", ALLOW, "shellInScope"),
        ("gh api repos/o/r/issues -f title=x", ASK, "risky:gh"),
        ("gh api repos/o/r", ALLOW, "shellInScope"),
        ("curl https://example.com", ALLOW, "shellInScope"),
        ("curl -X POST https://example.com", ASK, "risky:curl-upload"),
        ("curl -d a=1 https://example.com", ASK, "risky:curl-upload"),
        ("curl --json '{}' https://x.dev", ASK, "risky:curl-upload"),
        ("wget --post-data=a https://x.dev", ASK, "risky:curl-upload"),
        ("npm publish", ASK, "risky:publish"),
        ("cargo publish", ASK, "risky:publish"),
        ("docker push img", ASK, "risky:publish"),
        ("vercel --prod", ASK, "risky:deploy"),
        ("kubectl apply -f x.yaml", ASK, "risky:deploy"),
        ("kubectl get pods", ALLOW, "shellInScope"),
        ("terraform plan", ALLOW, "shellInScope"),
        ("terraform apply", ASK, "risky:deploy"),
        ("aws s3 ls", ASK, "risky:cloud"),
        ("sudo ls", ASK, "risky:system"),
        ("ssh host", ASK, "risky:system"),
        ("scp a host:b", ASK, "risky:system"),
        ("rsync -a a host:/b", ASK, "risky:system"),
        ("rsync -a a /work/project/b", ALLOW, "shellInScope"),
        ("open https://example.com", ASK, "risky:system"),
        ("osascript -e 'x'", ASK, "risky:system"),
        ("security find-generic-password -s x", ASK, "risky:system"),
        ("launchctl list", ASK, "risky:system"),
        ("defaults write com.x y 1", ASK, "risky:system"),
        ("brew install jq", ASK, "risky:install"),
        ("npm install -g x", ASK, "risky:install"),
        ("npm install", ALLOW, "shellInScope"),
        ("mail -s hi a@b.c", ASK, "risky:send"),
        ("rm -rf /work/project/build", ALLOW, "shellInScope"),
        ("rm -rf /work/project", ASK, "risky:rm-root"),
        ("rm -rf ~", ASK, "risky:rm-root"),
        ("rm /tmp/x", ASK, "outsideScope"),
        ("mv a.txt /tmp/", ASK, "outsideScope"),
        ("cp -r src /work/project/backup", ALLOW, "shellInScope"),
        ("ln -s /etc/passwd link", ALLOW, "shellInScope"),
        ("chmod 600 /Users/me/.ssh/config", ASK, "credentialPath"),
        ("echo hi > /tmp/out.txt", ASK, "outsideScope"),
        ("echo hi > notes.txt", ALLOW, "shellInScope"),
        ("echo hi >> /work/project/.env", ASK, "credentialPath"),
        ("cat /Users/me/.aws/credentials", ASK, "credentialPath"),
        ("echo $(whoami)", ASK, "shellSubstitution"),
        ("bash -c 'git push'", ASK, "risky:interpreter"),
        ("sh -c ls", ASK, "risky:interpreter"),
        ("eval \"$CMD\"", ASK, "risky:interpreter"),
        ("FOO=1 git push", ASK, "risky:git-push"),
        ("env FOO=1 git push", ASK, "risky:git-push"),
        ("nohup git push &", ASK, "risky:git-push"),
        ("xargs git push < list", ASK, "risky:git-push"),
        ("time cargo build", ALLOW, "shellInScope"),
        ("python3 script.py", ALLOW, "shellInScope"),
        ("cat <<EOF", ASK, "unparseable"),
        ("echo \"unterminated", ASK, "unparseable"),
        ("/usr/bin/git push", ASK, "risky:git-push"),
        ("cd /tmp && rm -rf x", ASK, "outsideScope"),
        // Every segment is checked, whatever joins them.
        ("", ALLOW, "shellInScope"),
        ("echo hi && git push", ASK, "risky:git-push"),
        ("ls; git push", ASK, "risky:git-push"),
        ("ls\ngit push", ASK, "risky:git-push"),
        ("true || git push", ASK, "risky:git-push"),
        ("ls | git push", ASK, "risky:git-push"),
        ("ls # && git push", ASK, "risky:git-push"),
        ("echo 'git push' \"rm -rf /\"", ALLOW, "shellInScope"),
        ("git commit -m \"git push later\"", ALLOW, "shellInScope"),
        // Quoting, escaping, keywords and wrappers do not hide the program.
        ("g\"i\"t push", ASK, "risky:git-push"),
        ("\\git push", ASK, "risky:git-push"),
        ("{ git push; }", ASK, "risky:git-push"),
        ("if true; then git push; fi", ASK, "risky:git-push"),
        ("for f in a b; do git push; done", ASK, "risky:git-push"),
        ("! git push", ASK, "risky:git-push"),
        ("command git push", ASK, "risky:git-push"),
        ("nice -n 10 git push", ASK, "risky:git-push"),
        ("timeout 60 git push", ASK, "risky:git-push"),
        ("timeout -s KILL 120 cargo test", ALLOW, "shellInScope"),
        ("xargs -n 1 -I {} git push < list", ASK, "risky:git-push"),
        ("npx --yes vercel --prod", ASK, "risky:deploy"),
        ("caffeinate -i git push", ASK, "risky:git-push"),
        ("env -S 'git push'", ASK, "unparseable"),
        ("nice --bogus git push", ASK, "unparseable"),
        ("$GIT push", ASK, "unresolvedPath"),
        ("${GIT:-git} push", ASK, "unresolvedPath"),
        ("gi? push", ASK, "unresolvedPath"),
        ("function f { ls; }", ASK, "unparseable"),
        ("(git push)", ASK, "unparseable"),
        ("f() { git push; }", ASK, "unparseable"),
        ("cat <<< hi", ASK, "unparseable"),
        ("diff <(ls) b", ASK, "shellSubstitution"),
        ("echo \"$(id)\"", ASK, "shellSubstitution"),
        ("echo `id`", ASK, "shellSubstitution"),
        // Interpreters and command runners.
        ("exec git push", ASK, "risky:interpreter"),
        ("source ./env.sh", ASK, "risky:interpreter"),
        (". ./env.sh", ASK, "risky:interpreter"),
        ("echo git push | sh", ASK, "risky:interpreter"),
        ("bash -s < script.sh", ASK, "risky:interpreter"),
        ("zsh -lc 'git push'", ASK, "risky:interpreter"),
        ("bash script.sh", ALLOW, "shellInScope"),
        (
            "find . -name '*.rs' -exec rm {} \\;",
            ASK,
            "risky:interpreter",
        ),
        ("find . -name '*.tmp'", ALLOW, "shellInScope"),
        ("fd -e rs -x wc -l", ASK, "risky:interpreter"),
        ("alias p='git push'", ASK, "risky:interpreter"),
        ("watch git status", ASK, "risky:interpreter"),
        ("pnpm dlx vercel", ASK, "risky:interpreter"),
        ("python3 -c 'print(1)'", ALLOW, "shellInScope"),
        ("node -e 'console.log(1)'", ALLOW, "shellInScope"),
        ("make test", ALLOW, "shellInScope"),
        // Git.
        ("git --no-pager push", ASK, "risky:git-push"),
        ("git push --tags", ASK, "risky:git-push"),
        ("git -c alias.p=push p", ASK, "risky:interpreter"),
        ("git --bogus status", ASK, "unparseable"),
        ("git log --oneline -5", ALLOW, "shellInScope"),
        ("git --version", ALLOW, "shellInScope"),
        ("git clean -n", ALLOW, "shellInScope"),
        ("git clean --force", ASK, "risky:git-clean"),
        ("git branch -d feature", ALLOW, "shellInScope"),
        (
            "git branch --delete --force feature",
            ASK,
            "risky:git-branch-delete",
        ),
        ("git tag -d v1", ASK, "risky:git-tag-delete"),
        ("git tag v1", ALLOW, "shellInScope"),
        ("git reset --soft HEAD~1", ALLOW, "shellInScope"),
        ("git send-email x.patch", ASK, "risky:send"),
        ("git config --global user.name x", ASK, "risky:system"),
        ("git config user.name x", ASK, "risky:agent-config"),
        ("git rebase -x 'make test' main", ASK, "risky:interpreter"),
        ("git submodule foreach git push", ASK, "risky:interpreter"),
        ("git bisect run ./test.sh", ASK, "risky:interpreter"),
        // GitHub and GitLab.
        ("gh issue create -t x", ASK, "risky:gh"),
        ("gh pr merge 1", ASK, "risky:gh"),
        ("gh pr comment 1 -b x", ASK, "risky:gh"),
        ("gh repo delete o/r", ASK, "risky:gh"),
        ("gh release create v1", ASK, "risky:gh"),
        ("gh issue list", ALLOW, "shellInScope"),
        ("gh -R o/r pr list", ALLOW, "shellInScope"),
        ("gh run view 1 --log", ALLOW, "shellInScope"),
        ("gh pr view 12 --web", ASK, "risky:gh"),
        ("gh search issues bug", ALLOW, "shellInScope"),
        ("gh browse", ASK, "risky:gh"),
        ("gh api -X DELETE repos/o/r", ASK, "risky:gh"),
        ("gh api --method=PATCH repos/o/r", ASK, "risky:gh"),
        (
            "gh api --method GET search/issues -f q=x",
            ALLOW,
            "shellInScope",
        ),
        ("gh api graphql -f query=x", ASK, "risky:gh"),
        ("gh api --paginate repos/o/r/pulls", ALLOW, "shellInScope"),
        ("glab mr create", ASK, "risky:glab"),
        ("glab mr list", ALLOW, "shellInScope"),
        ("glab api -X POST projects/1/issues", ASK, "risky:glab"),
        // Uploads and mail.
        (
            "curl -sSL https://example.com -o page.html",
            ALLOW,
            "shellInScope",
        ),
        ("curl -XPOST https://x.dev", ASK, "risky:curl-upload"),
        ("curl -X GET https://x.dev", ALLOW, "shellInScope"),
        ("curl -XGET https://x.dev", ALLOW, "shellInScope"),
        ("curl --request=PUT https://x.dev", ASK, "risky:curl-upload"),
        ("curl -sd a=1 https://x.dev", ASK, "risky:curl-upload"),
        ("curl -F file=@a https://x.dev", ASK, "risky:curl-upload"),
        ("curl -T a https://x.dev", ASK, "risky:curl-upload"),
        (
            "curl --data-binary @a https://x.dev",
            ASK,
            "risky:curl-upload",
        ),
        ("curl -K opts.txt", ASK, "risky:curl-upload"),
        (
            "curl smtp://mail.x.dev --mail-rcpt a@b.c",
            ASK,
            "risky:curl-upload",
        ),
        ("curl -I https://x.dev", ALLOW, "shellInScope"),
        ("curl -o /tmp/x https://x.dev", ASK, "outsideScope"),
        ("curl -so/tmp/x https://x.dev", ASK, "outsideScope"),
        ("wget https://x.dev/a.tar.gz", ALLOW, "shellInScope"),
        ("wget --method=PUT https://x.dev", ASK, "risky:curl-upload"),
        (
            "wget --body-file req.json https://x.dev",
            ASK,
            "risky:curl-upload",
        ),
        ("wget -qO /tmp/x https://x.dev", ASK, "outsideScope"),
        ("sendmail a@b.c < msg", ASK, "risky:send"),
        ("nc example.com 80 < req", ASK, "risky:send"),
        // Publishing, installs, deploys and cloud.
        ("yarn publish", ASK, "risky:publish"),
        ("pnpm publish --access public", ASK, "risky:publish"),
        ("npm unpublish x", ASK, "risky:publish"),
        ("npm --workspace a publish", ASK, "risky:publish"),
        ("npm i -g x", ASK, "risky:install"),
        ("npm install --global x", ASK, "risky:install"),
        ("yarn global add x", ASK, "risky:install"),
        ("pnpm add -g x", ASK, "risky:install"),
        ("npm run build", ALLOW, "shellInScope"),
        ("npm test", ALLOW, "shellInScope"),
        ("cargo install ripgrep", ASK, "risky:install"),
        ("cargo build --release", ALLOW, "shellInScope"),
        ("gem push x.gem", ASK, "risky:publish"),
        ("twine upload dist/*", ASK, "risky:publish"),
        (
            "go install golang.org/x/tools/gopls@latest",
            ASK,
            "risky:install",
        ),
        ("pip install -r requirements.txt", ALLOW, "shellInScope"),
        ("docker build -t x .", ALLOW, "shellInScope"),
        ("docker buildx build --push -t x .", ASK, "risky:publish"),
        ("podman push img", ASK, "risky:publish"),
        ("kubectl delete pod x", ASK, "risky:deploy"),
        (
            "kubectl -n prod rollout restart deploy/x",
            ASK,
            "risky:deploy",
        ),
        ("kubectl -n prod get pods", ALLOW, "shellInScope"),
        ("helm upgrade x ./chart", ASK, "risky:deploy"),
        ("helm list", ALLOW, "shellInScope"),
        ("terraform destroy", ASK, "risky:deploy"),
        ("terraform -chdir=infra apply", ASK, "risky:deploy"),
        ("pulumi up", ASK, "risky:deploy"),
        ("pulumi preview", ALLOW, "shellInScope"),
        ("netlify deploy --prod", ASK, "risky:deploy"),
        ("fly deploy", ASK, "risky:deploy"),
        ("flyctl status", ALLOW, "shellInScope"),
        ("heroku releases", ALLOW, "shellInScope"),
        ("firebase deploy", ASK, "risky:deploy"),
        ("vercel", ASK, "risky:deploy"),
        ("vercel dev", ALLOW, "shellInScope"),
        ("gcloud compute instances list", ASK, "risky:cloud"),
        ("az login", ASK, "risky:cloud"),
        ("stripe charges create", ASK, "risky:cloud"),
        ("brew list", ALLOW, "shellInScope"),
        ("brew upgrade", ASK, "risky:install"),
        // System and access.
        ("defaults read com.apple.dock", ALLOW, "shellInScope"),
        ("defaults delete com.x", ASK, "risky:system"),
        ("crontab -l", ASK, "risky:system"),
        ("dd if=/dev/zero of=x", ASK, "risky:system"),
        ("sudo -u me ls", ASK, "risky:system"),
        ("rsync -av src/ user@host:/srv/", ASK, "risky:system"),
        ("rsync -a -e ssh a b", ASK, "risky:system"),
        ("rsync -a a /tmp/b", ASK, "outsideScope"),
        (
            "rsync -a --remove-source-files /tmp/a b",
            ASK,
            "outsideScope",
        ),
        // Destructive file commands.
        ("rm -rf .", ASK, "risky:rm-root"),
        ("rm -rf /", ASK, "risky:rm-root"),
        ("rm -fr /work/agents/Bot", ASK, "risky:rm-root"),
        ("rm -r -f ~/", ASK, "risky:rm-root"),
        ("rm --recursive /work/project/", ASK, "risky:rm-root"),
        ("rm -rf /work/project/*", ASK, "risky:rm-root"),
        ("rm -rf *", ASK, "risky:rm-root"),
        ("rm -rf build/*", ALLOW, "shellInScope"),
        ("rm -rf /work", ASK, "outsideScope"),
        ("rm -rf ../Other", ASK, "outsideScope"),
        ("rm -rf /work/projectile", ASK, "outsideScope"),
        ("rm -rf $DIR", ASK, "unresolvedPath"),
        ("rm -rf \"$HOME\"/x", ASK, "unresolvedPath"),
        ("rm -rf ~root", ASK, "unresolvedPath"),
        ("rm -rf {/tmp/x,y}", ASK, "unresolvedPath"),
        ("rm -- -rf", ALLOW, "shellInScope"),
        ("rm /work/project/.env", ASK, "credentialPath"),
        ("rmdir /tmp/d", ASK, "outsideScope"),
        ("unlink /tmp/x", ASK, "outsideScope"),
        ("truncate -s 0 /tmp/x", ASK, "outsideScope"),
        ("touch /tmp/x", ASK, "outsideScope"),
        ("touch a b c", ALLOW, "shellInScope"),
        ("mkdir -p /work/project/a/b", ALLOW, "shellInScope"),
        ("mkdir /tmp/d", ASK, "outsideScope"),
        ("mv /tmp/x ./y", ASK, "outsideScope"),
        ("mv old.txt new.txt", ALLOW, "shellInScope"),
        (
            "mv /work/project /work/agents/Bot/old",
            ASK,
            "risky:rm-root",
        ),
        ("mv *.txt archive/", ALLOW, "shellInScope"),
        ("sed -i '' 's/a/b/' ~/.zshrc", ASK, "outsideScope"),
        ("sed -i.bak -e 's/a/b/' /tmp/x", ASK, "outsideScope"),
        ("sed -i '' 's/a/b/' src/main.rs", ALLOW, "shellInScope"),
        ("sed -n 's/a/b/p' /etc/hosts", ALLOW, "shellInScope"),
        ("ditto src /tmp/copy", ASK, "outsideScope"),
        ("cp /etc/hosts ./hosts", ASK, "outsideScope"),
        ("cp ~/.ssh/id_rsa ./key", ASK, "credentialPath"),
        ("cp -t /tmp a b", ASK, "outsideScope"),
        ("cp --target-directory=/tmp a", ASK, "outsideScope"),
        (
            "install -m 755 tool /usr/local/bin/tool",
            ASK,
            "outsideScope",
        ),
        ("ln -s ~/.ssh/id_rsa k", ASK, "credentialPath"),
        ("ln -s /etc d && echo x > d/passwd", ASK, "unresolvedPath"),
        ("ln -s /tmp t && cd t && rm -rf x", ASK, "unresolvedPath"),
        ("chmod +x /work/project/run.sh", ALLOW, "shellInScope"),
        ("chmod 644 /tmp/x", ASK, "outsideScope"),
        ("chmod --reference=a /tmp/x", ASK, "outsideScope"),
        ("chown -R me /work/project/a", ALLOW, "shellInScope"),
        ("tee -a /tmp/log", ASK, "outsideScope"),
        (
            "cargo test 2>&1 | tee /work/project/log.txt",
            ALLOW,
            "shellInScope",
        ),
        // Redirects.
        ("echo hi > /dev/null", ALLOW, "shellInScope"),
        ("ls &> /tmp/out", ASK, "outsideScope"),
        ("ls 2> /work/project/.env", ASK, "credentialPath"),
        ("echo hi >| /tmp/x", ASK, "outsideScope"),
        ("echo x <> /tmp/x", ASK, "outsideScope"),
        ("echo hi > \"$OUT\"", ASK, "unresolvedPath"),
        ("echo hi > ../escape.txt", ASK, "outsideScope"),
        ("echo hi > ''", ASK, "unresolvedPath"),
        // Credential material in any argument or input.
        ("cat < ~/.ssh/id_rsa", ASK, "credentialPath"),
        ("sort < /etc/hosts", ASK, "outsideScope"),
        // Reading programs' operands stay in scope, like write targets.
        ("cat ~/Documents/notes.txt", ASK, "outsideScope"),
        ("cat -n /etc/hosts", ASK, "outsideScope"),
        ("cat --number /etc/hosts", ASK, "outsideScope"),
        ("head -n 5 /etc/hosts", ASK, "outsideScope"),
        ("head -n 5 notes.txt", ALLOW, "shellInScope"),
        ("tail --lines=5 /etc/hosts", ASK, "outsideScope"),
        ("cut -d / -f 2 notes.txt", ALLOW, "shellInScope"),
        ("stat -f %z /etc/hosts", ASK, "outsideScope"),
        ("wc -l < /etc/hosts", ASK, "outsideScope"),
        ("ls ~", ASK, "outsideScope"),
        ("ls -la", ALLOW, "shellInScope"),
        ("du -sh ..", ASK, "outsideScope"),
        ("cat /work/project/README.md", ALLOW, "shellInScope"),
        ("cat $HOME/notes.txt", ASK, "unresolvedPath"),
        ("cp ~/Documents/a.txt ./a.txt", ASK, "outsideScope"),
        (
            "cp notes.txt /work/project/notes.txt",
            ALLOW,
            "shellInScope",
        ),
        ("grep -r TODO /work/project/src", ALLOW, "shellInScope"),
        ("head -5 /Users/me/.netrc", ASK, "credentialPath"),
        ("base64 ~/.ssh/id_ed25519", ASK, "credentialPath"),
        (
            "tar czf out.tgz --files-from=/Users/me/.aws/config",
            ASK,
            "credentialPath",
        ),
        ("KEY=~/.ssh/id_rsa ls", ASK, "credentialPath"),
        ("cd ~/.ssh && cat id_rsa", ASK, "credentialPath"),
        // zsh forms and builtins that run or hide commands.
        ("echo x >! /tmp/x", ASK, "outsideScope"),
        ("while true { git push }", ASK, "unparseable"),
        ("if [[ -f x ]] { git push }", ASK, "unparseable"),
        // zsh's short forms run the command written straight after `]]`.
        ("if [[ -n x ]] git push origin main", ASK, "unparseable"),
        (
            "while [[ -n x ]] rm -rf /Users/me/Documents",
            ASK,
            "unparseable",
        ),
        ("until [[ -n x ]] git push", ASK, "unparseable"),
        ("if [[ -n x ]] then git push; fi", ASK, "unparseable"),
        ("if [[ -n x ]] ls ]]", ASK, "unparseable"),
        ("for f (a b) git push", ASK, "unparseable"),
        ("if (( 1 )) git push", ASK, "unparseable"),
        ("[[ -f x ]] && ls", ALLOW, "shellInScope"),
        ("if [[ -f x ]]; then ls; fi", ALLOW, "shellInScope"),
        ("if [[ -f x ]] && git push", ASK, "risky:git-push"),
        // Runners that name the command they run.
        ("xcrun git push origin main", ASK, "risky:git-push"),
        ("xcrun --sdk macosx git push", ASK, "risky:git-push"),
        ("xcrun --show-sdk-path", ALLOW, "shellInScope"),
        ("xcrun swift build", ALLOW, "shellInScope"),
        ("taskpolicy -b git push origin main", ASK, "risky:git-push"),
        ("taskpolicy -c utility rm -rf /tmp/x", ASK, "outsideScope"),
        ("uv run git push origin main", ASK, "risky:git-push"),
        (
            "uv --quiet run --with requests git push",
            ASK,
            "risky:git-push",
        ),
        ("uv run -- git push", ASK, "risky:git-push"),
        ("uv run --frozen pytest", ALLOW, "shellInScope"),
        ("uv run --unknown-option git push", ASK, "unparseable"),
        ("uv sync", ALLOW, "shellInScope"),
        ("poetry run rm -rf /tmp/x", ASK, "outsideScope"),
        ("poetry run pytest", ALLOW, "shellInScope"),
        ("pdm run git push", ASK, "risky:git-push"),
        ("hatch run git push", ASK, "risky:git-push"),
        ("hatch run test:git push", ASK, "risky:git-push"),
        ("hatch run test:cov", ALLOW, "shellInScope"),
        ("hatch run +py=3.12 git push", ASK, "unparseable"),
        ("pipenv run git push", ASK, "risky:git-push"),
        ("lockf /tmp/lock git push", ASK, "risky:interpreter"),
        ("leaks --atExit -- git push", ASK, "risky:interpreter"),
        (
            "xcrun xctrace record --launch -- git push",
            ASK,
            "risky:interpreter",
        ),
        ("noglob git push", ASK, "risky:git-push"),
        ("repeat 3 git push", ASK, "risky:git-push"),
        ("arch -arm64 git push", ASK, "risky:git-push"),
        ("timeout {1,git} push", ASK, "unresolvedPath"),
        (
            "mapfile -C 'git push' -c 1 < list",
            ASK,
            "risky:interpreter",
        ),
        ("zmodload zsh/net/tcp", ASK, "risky:interpreter"),
        ("strace git push", ASK, "risky:interpreter"),
        ("socat - TCP:x.dev:80", ASK, "risky:send"),
        ("openssl s_client -connect x.dev:443", ASK, "risky:send"),
        ("openssl rand -hex 16", ALLOW, "shellInScope"),
        ("rclone copy . remote:bucket", ASK, "risky:cloud"),
        ("ngrok http 3000", ASK, "risky:system"),
        ("http POST x.dev a=1", ASK, "risky:curl-upload"),
        // Expansions that rewrite a risky program's arguments.
        ("git {push,}", ASK, "unresolvedPath"),
        ("git $SUB", ASK, "unresolvedPath"),
        ("S=push; git $S", ASK, "unresolvedPath"),
        ("git reset --{hard,}", ASK, "unresolvedPath"),
        (
            "kubectl get pods -o jsonpath='{.items[*].metadata.name}'",
            ALLOW,
            "shellInScope",
        ),
        ("echo $HOME", ALLOW, "shellInScope"),
        ("awk '{print $1}' data.txt", ALLOW, "shellInScope"),
        ("CDPATH=/tmp cd x && rm -rf y", ASK, "unresolvedPath"),
        ("export HOME=/tmp", ASK, "unresolvedPath"),
        // Hard links share content with their source.
        ("ln /etc/hosts h", ASK, "outsideScope"),
        ("cp -al /tmp/src dst", ASK, "outsideScope"),
        // More git, hub, transfer, package and container cases.
        ("git credential fill", ASK, "credentialPath"),
        ("git lfs push origin main", ASK, "risky:git-push"),
        ("gh auth token", ASK, "risky:gh"),
        ("gh auth status --show-token", ASK, "risky:gh"),
        ("curl -D /tmp/h https://x.dev", ASK, "outsideScope"),
        (
            "curl --cookie-jar /tmp/c https://x.dev",
            ASK,
            "outsideScope",
        ),
        ("wget --config=x.rc https://x.dev", ASK, "risky:curl-upload"),
        ("npm config set //r/:_authToken x", ASK, "outsideScope"),
        ("docker login", ASK, "risky:publish"),
        ("docker run --privileged img", ASK, "risky:system"),
        ("docker run -v /:/host img", ASK, "outsideScope"),
        // Every container and tar front end shares one family table.
        ("nerdctl run -v /:/h img", ASK, "outsideScope"),
        ("podman run -v /:/h img", ASK, "outsideScope"),
        ("nerdctl run -v .:/app img", ALLOW, "shellInScope"),
        ("gtar -xf a.tar $X", ASK, "unresolvedPath"),
        ("bsdtar -xf a.tar $X", ASK, "unresolvedPath"),
        ("tar -xf a.tar $X", ASK, "unresolvedPath"),
        ("gtar -xf a.tar -C /tmp/x", ASK, "outsideScope"),
        ("echo x | xargs gtar", ASK, "risky:xargs"),
        ("echo x | xargs bsdtar", ASK, "risky:xargs"),
        ("gtar -xf a.tar", ALLOW, "shellInScope"),
        (
            "docker run --mount type=bind,source=/Users/me,target=/h img",
            ASK,
            "outsideScope",
        ),
        ("docker run -v .:/app img", ALLOW, "shellInScope"),
        ("docker run -v cache:/cache img", ALLOW, "shellInScope"),
        ("pip install --user x", ASK, "risky:install"),
        ("pipx install ruff", ASK, "risky:install"),
        ("uv tool install ruff", ASK, "risky:install"),
        ("uv run pytest", ALLOW, "shellInScope"),
        ("rustup component add clippy", ASK, "risky:install"),
        ("rustup show", ALLOW, "shellInScope"),
        // Agents never rewrite their own harness configuration, even in scope.
        (
            "echo '{}' > .claude/settings.json",
            ASK,
            "risky:agent-config",
        ),
        (
            "cp x.toml /work/project/.codex/config.toml",
            ASK,
            "risky:agent-config",
        ),
        ("rm -rf .codex", ASK, "risky:agent-config"),
        ("sed -i '' 's/a/b/' .mcp.json", ASK, "risky:agent-config"),
        (
            "echo x > /work/project/.CLAUDE/settings.json",
            ASK,
            "risky:agent-config",
        ),
        ("touch /Users/me/.grok/config.toml", ASK, "credentialPath"),
        ("echo x > .claude/.env", ASK, "credentialPath"),
        ("cat .claude/settings.json .mcp.json", ALLOW, "shellInScope"),
        (
            "cp .claude/settings.json backup.json",
            ALLOW,
            "shellInScope",
        ),
        // Environment variables that make a later program run a command.
        (
            "GIT_SSH_COMMAND='ssh -o ProxyCommand=x' git fetch",
            ASK,
            "risky:interpreter",
        ),
        ("PAGER='git push' git log", ASK, "risky:interpreter"),
        (
            "env NODE_OPTIONS=--require=./x.js npm test",
            ASK,
            "risky:interpreter",
        ),
        ("export EDITOR=vim; git commit", ASK, "risky:interpreter"),
        ("DYLD_INSERT_LIBRARIES=x ls", ASK, "risky:interpreter"),
        ("FOO=1 cargo test", ALLOW, "shellInScope"),
        ("EDITOR=vim", ASK, "risky:interpreter"),
        (
            "declare -x GIT_EXTERNAL_DIFF=./d.sh",
            ASK,
            "risky:interpreter",
        ),
        ("typeset -x VISUAL=./v.sh", ASK, "risky:interpreter"),
        ("LD_PRELOAD+=x.so ls", ASK, "risky:interpreter"),
        (
            "npm_config_script_shell=./x npm test",
            ASK,
            "risky:interpreter",
        ),
        (
            "NPM_CONFIG_SCRIPT_SHELL=./x npm test",
            ASK,
            "risky:interpreter",
        ),
        (
            "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=./x cargo test",
            ASK,
            "risky:interpreter",
        ),
        (
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.pager GIT_CONFIG_VALUE_0=x git log",
            ASK,
            "risky:interpreter",
        ),
        (
            "GIT_CONFIG_KEY_0=core.pager git log",
            ASK,
            "risky:interpreter",
        ),
        ("RUSTC_WRAPPER=./w cargo build", ASK, "risky:interpreter"),
        (
            "nice -n 5 env PYTHONSTARTUP=x.py python3",
            ASK,
            "risky:interpreter",
        ),
        (
            "export RUST_BACKTRACE=1 && cargo test",
            ALLOW,
            "shellInScope",
        ),
        ("echo PAGER=less", ALLOW, "shellInScope"),
        ("make CFLAGS=-O2", ALLOW, "shellInScope"),
        // Git hooks and repository configuration run on the next git command.
        (
            "printf x > /work/project/.git/hooks/pre-commit",
            ASK,
            "risky:agent-config",
        ),
        (
            "chmod 777 /work/project/.git/hooks/pre-commit",
            ASK,
            "risky:agent-config",
        ),
        (
            "git config --local core.hooksPath /tmp/x",
            ASK,
            "risky:agent-config",
        ),
        ("git config user.email a@b.c", ASK, "risky:agent-config"),
        ("git config --get user.email", ALLOW, "shellInScope"),
        ("git config --global alias.p push", ASK, "risky:system"),
        (
            "git -C /work/project config core.pager x",
            ASK,
            "risky:agent-config",
        ),
        ("git config --system core.editor x", ASK, "risky:system"),
        (
            "git config --worktree core.fsmonitor ./x",
            ASK,
            "risky:agent-config",
        ),
        (
            "git config --file .git/config core.pager x",
            ASK,
            "risky:agent-config",
        ),
        ("git config --unset core.pager", ASK, "risky:agent-config"),
        ("git config --edit", ASK, "risky:agent-config"),
        ("git config set core.pager x", ASK, "risky:agent-config"),
        ("git config --global --unset alias.p", ASK, "risky:system"),
        ("git config", ASK, "risky:agent-config"),
        ("git config user.email", ASK, "risky:agent-config"),
        ("git config --list --show-origin", ALLOW, "shellInScope"),
        ("git config -l", ALLOW, "shellInScope"),
        (
            "git config --global --get-regexp alias",
            ALLOW,
            "shellInScope",
        ),
        (
            "git config --get-all remote.origin.fetch",
            ALLOW,
            "shellInScope",
        ),
        ("git config get user.email", ALLOW, "shellInScope"),
        ("git config list", ALLOW, "shellInScope"),
        ("git config --get --add a.b c", ASK, "risky:agent-config"),
        ("cp hook.sh .git/hooks/pre-push", ASK, "risky:agent-config"),
        (
            "ln -sf ../../hook.sh /work/project/.git/hooks/post-checkout",
            ASK,
            "risky:agent-config",
        ),
        ("rm -rf /work/project/.git/hooks", ASK, "risky:agent-config"),
        (
            "echo '[core]' >> /work/project/.git/config",
            ASK,
            "risky:agent-config",
        ),
        (
            "rsync -a evil/ /work/project/.git/",
            ASK,
            "risky:agent-config",
        ),
        (
            "echo x > /work/project/.git/modules/sub/hooks/pre-commit",
            ASK,
            "risky:agent-config",
        ),
        (
            "echo x > /work/project/.git/worktrees/w/config.worktree",
            ASK,
            "risky:agent-config",
        ),
        ("git init --template=/tmp/t", ASK, "risky:agent-config"),
        (
            "git clone --template /tmp/t https://x.dev/r.git",
            ASK,
            "risky:agent-config",
        ),
        (
            "git clone -c core.hooksPath=h https://x.dev/r.git",
            ASK,
            "risky:interpreter",
        ),
        (
            "git clone --config=core.pager=x https://x.dev/r.git",
            ASK,
            "risky:interpreter",
        ),
        (
            "git clone -u ./x https://x.dev/r.git",
            ASK,
            "risky:interpreter",
        ),
        (
            "git fetch --upload-pack=./x origin",
            ASK,
            "risky:interpreter",
        ),
        ("git clone https://x.dev/r.git", ALLOW, "shellInScope"),
        (
            "cat /work/project/.git/hooks/pre-commit.sample",
            ALLOW,
            "shellInScope",
        ),
        ("echo x > /work/project/.gitignore", ALLOW, "shellInScope"),
        (
            "echo x > /work/project/hooks/pre-commit",
            ALLOW,
            "shellInScope",
        ),
        ("echo x > /work/project/config", ALLOW, "shellInScope"),
        // Credential stores the shared tables do not name.
        ("cat ~/.config/gh/hosts.yml", ASK, "credentialPath"),
        ("ls /Users/me/Library/Keychains", ASK, "credentialPath"),
        ("tar czf x.tgz ~/.config/gcloud", ASK, "credentialPath"),
        ("cat ~/.password-store/email.gpg", ASK, "credentialPath"),
        (
            "cp -r /Users/me/.local/share/keyrings ./k",
            ASK,
            "credentialPath",
        ),
        ("cat /opt/gh/hosts.yml", ASK, "credentialPath"),
        ("cat /Users/me/.config/ghostty/config", ASK, "outsideScope"),
        // --- Fix round 2: adversarial-review classes ---
        // 1. Program and wrapper names fold case (macOS is case-insensitive).
        ("GIT push origin main", ASK, "risky:git-push"),
        ("BASH -c 'git push'", ASK, "risky:interpreter"),
        ("ZSH -c 'git push'", ASK, "risky:interpreter"),
        ("SH -c 'git push'", ASK, "risky:interpreter"),
        ("DASH -c 'git push'", ASK, "risky:interpreter"),
        ("CURL -d a=1 https://example.com", ASK, "risky:curl-upload"),
        ("CURL -X POST https://example.com", ASK, "risky:curl-upload"),
        (
            "CURL --upload-file a https://example.com",
            ASK,
            "risky:curl-upload",
        ),
        ("WGET --post-data=a https://x.dev", ASK, "risky:curl-upload"),
        ("SSH host", ASK, "risky:system"),
        ("SCP a host:b", ASK, "risky:system"),
        ("SUDO ls", ASK, "risky:system"),
        ("OSASCRIPT -e 'x'", ASK, "risky:system"),
        ("OPEN https://example.com", ASK, "risky:system"),
        ("NPM publish", ASK, "risky:publish"),
        ("CARGO publish", ASK, "risky:publish"),
        ("DOCKER push img", ASK, "risky:publish"),
        ("VERCEL --prod", ASK, "risky:deploy"),
        ("KUBECTL apply -f x.yaml", ASK, "risky:deploy"),
        ("TERRAFORM apply", ASK, "risky:deploy"),
        ("AWS s3 ls", ASK, "risky:cloud"),
        ("BREW install jq", ASK, "risky:install"),
        ("GH pr create --fill", ASK, "risky:gh"),
        ("MAIL -s hi a@b.c", ASK, "risky:send"),
        ("LAUNCHCTL list", ASK, "risky:system"),
        ("DEFAULTS write com.x y 1", ASK, "risky:system"),
        ("RSYNC -a -e ssh a b", ASK, "risky:system"),
        ("NOHUP git push origin main", ASK, "risky:git-push"),
        ("ENV git push origin main", ASK, "risky:git-push"),
        ("NICE git push", ASK, "risky:git-push"),
        ("TIME git push", ASK, "risky:git-push"),
        ("COMMAND git push", ASK, "risky:git-push"),
        ("XARGS git push", ASK, "risky:git-push"),
        // 2. ssh/ftp/mail/rpc family names.
        ("slogin host", ASK, "risky:system"),
        ("mosh host", ASK, "risky:system"),
        ("autossh host", ASK, "risky:system"),
        ("sshpass -p p ssh host", ASK, "risky:system"),
        ("lftp host", ASK, "risky:send"),
        ("ncftp host", ASK, "risky:send"),
        ("tftp host", ASK, "risky:send"),
        ("wget2 --method=POST https://x", ASK, "risky:curl-upload"),
        ("swaks --to a@b.c --server mail.x", ASK, "risky:send"),
        ("mutt -s hi a@b.c", ASK, "risky:send"),
        ("neomutt -s hi a@b.c", ASK, "risky:send"),
        (
            "grpcurl -d '{}' localhost:443 svc.Method",
            ASK,
            "risky:send",
        ),
        ("grpcurl list", ALLOW, "shellInScope"),
        ("wscat -c wss://x", ASK, "risky:send"),
        ("mosquitto_pub -t t -m m -h host", ASK, "risky:send"),
        ("busybox sh -c 'git push'", ASK, "risky:interpreter"),
        (
            "busybox wget --post-data=a https://x",
            ASK,
            "risky:curl-upload",
        ),
        ("busybox nc host 80", ASK, "risky:send"),
        ("gio open https://x", ASK, "risky:system"),
        // 3. xargs supplies hidden argv to argument-sensitive programs.
        ("echo push origin main | xargs git", ASK, "risky:xargs"),
        ("xargs git", ASK, "risky:xargs"),
        ("echo push | xargs -I {} git {}", ASK, "risky:xargs"),
        (
            "echo -d a=1 https://example.com | xargs curl",
            ASK,
            "risky:xargs",
        ),
        ("echo publish | xargs npm", ASK, "risky:xargs"),
        ("echo publish | xargs cargo", ASK, "risky:xargs"),
        ("echo push img | xargs docker", ASK, "risky:xargs"),
        ("echo apply -f x.yaml | xargs kubectl", ASK, "risky:xargs"),
        ("echo pr create --fill | xargs gh", ASK, "risky:xargs"),
        ("echo install jq | xargs brew", ASK, "risky:xargs"),
        ("echo push origin main | XARGS git", ASK, "risky:xargs"),
        ("echo a | xargs echo", ALLOW, "shellInScope"),
        ("ls | xargs cat", ASK, "unresolvedPath"),
        // 4. git plumbing, alias/env config, hook run.
        (
            "git send-pack origin refs/heads/main",
            ASK,
            "risky:git-push",
        ),
        (
            "git http-push http://example.com/repo.git main",
            ASK,
            "risky:git-push",
        ),
        ("git svn dcommit", ASK, "risky:git-push"),
        ("git p4 submit", ASK, "risky:git-push"),
        ("git-push origin main", ASK, "risky:git-push"),
        (
            "/opt/libexec/git-core/git-push origin main",
            ASK,
            "risky:git-push",
        ),
        ("git config alias.p push", ASK, "risky:agent-config"),
        ("git hook run pre-commit", ASK, "risky:interpreter"),
        // 5. npx/pnpx/bunx/corepack/bundle unwrap and version/scope strip.
        ("npx --yes vercel@latest --prod", ASK, "risky:deploy"),
        ("npx --yes @vercel/cli --prod", ASK, "risky:deploy"),
        ("npx --yes npm@latest publish", ASK, "risky:publish"),
        ("pnpx vercel@latest --prod", ASK, "risky:deploy"),
        ("bunx vercel@latest --prod", ASK, "risky:deploy"),
        ("npx --yes wrangler deploy", ASK, "risky:deploy"),
        ("corepack npm publish", ASK, "risky:publish"),
        ("bundle exec gem push x.gem", ASK, "risky:publish"),
        ("npx --yes eslint .", ALLOW, "shellInScope"),
        ("bundle exec rspec", ALLOW, "shellInScope"),
        // 6. publish/install tools.
        ("uv publish", ASK, "risky:publish"),
        ("poetry publish", ASK, "risky:publish"),
        ("pdm publish", ASK, "risky:publish"),
        ("hatch publish", ASK, "risky:publish"),
        ("flit publish", ASK, "risky:publish"),
        ("mvn deploy", ASK, "risky:publish"),
        ("gradle publish", ASK, "risky:publish"),
        ("./gradlew publish", ASK, "risky:publish"),
        ("dotnet nuget push pkg.nupkg", ASK, "risky:publish"),
        ("cargo binstall ripgrep", ASK, "risky:install"),
        ("npm install --prefix /usr/local x", ASK, "outsideScope"),
        ("pip install -t /usr/local x", ASK, "outsideScope"),
        ("pip install -t ./vendor x", ALLOW, "shellInScope"),
        ("mvn test", ALLOW, "shellInScope"),
        ("gradle build", ALLOW, "shellInScope"),
        ("poetry install", ALLOW, "shellInScope"),
        ("uv sync", ALLOW, "shellInScope"),
        // 7. deploy CLIs.
        ("wrangler deploy", ASK, "risky:deploy"),
        ("serverless deploy", ASK, "risky:deploy"),
        ("sls deploy", ASK, "risky:deploy"),
        ("cdk deploy", ASK, "risky:deploy"),
        ("cdk destroy", ASK, "risky:deploy"),
        ("sam deploy", ASK, "risky:deploy"),
        ("kamal deploy", ASK, "risky:deploy"),
        ("ansible-playbook playbook.yml", ASK, "risky:deploy"),
        ("helmfile apply", ASK, "risky:deploy"),
        ("kapp deploy -a x -f .", ASK, "risky:deploy"),
        ("argocd app sync x", ASK, "risky:deploy"),
        ("flux reconcile kustomization x", ASK, "risky:deploy"),
        ("supabase db push", ASK, "risky:deploy"),
        ("railway up", ASK, "risky:deploy"),
        ("eb deploy", ASK, "risky:deploy"),
        ("amplify push", ASK, "risky:deploy"),
        ("oc apply -f x.yaml", ASK, "risky:deploy"),
        ("wrangler dev", ALLOW, "shellInScope"),
        ("cdk diff", ALLOW, "shellInScope"),
        ("argocd app list", ALLOW, "shellInScope"),
        ("supabase status", ALLOW, "shellInScope"),
        ("oc get pods", ALLOW, "shellInScope"),
        // 8. image publish without `docker push`.
        (
            "docker buildx build --output type=registry,ref=img .",
            ASK,
            "risky:publish",
        ),
        (
            "docker build --output type=image,push=true .",
            ASK,
            "risky:publish",
        ),
        ("nerdctl push img", ASK, "risky:publish"),
        ("buildah push img", ASK, "risky:publish"),
        (
            "skopeo copy containers-storage:img docker://registry/img",
            ASK,
            "risky:publish",
        ),
        ("oras push registry/img:t .", ASK, "risky:publish"),
        ("crane push img registry/img", ASK, "risky:publish"),
        (
            "docker buildx build --output type=local,dest=out .",
            ALLOW,
            "shellInScope",
        ),
        ("skopeo inspect docker://img", ALLOW, "shellInScope"),
        // 9. system and interpreter-like tools.
        (
            "expect -c 'spawn ssh host; interact'",
            ASK,
            "risky:interpreter",
        ),
        ("tmux send-keys 'git push' Enter", ASK, "risky:interpreter"),
        ("screen -X stuff 'git push\n'", ASK, "risky:interpreter"),
        ("echo git push | at now", ASK, "risky:interpreter"),
        (
            "plutil -insert foo -string bar /Users/me/Library/Preferences/com.apple.dock.plist",
            ASK,
            "risky:system",
        ),
        (
            "/usr/libexec/PlistBuddy -c 'Set :x 1' /Users/me/Library/Preferences/com.x.plist",
            ASK,
            "risky:system",
        ),
        ("mount -t nfs host:/ /mnt", ASK, "risky:system"),
        ("umount /mnt", ASK, "risky:system"),
        ("sshfs host: /mnt", ASK, "risky:system"),
        ("dscl . -passwd /Users/me", ASK, "risky:system"),
        ("nvram boot-args=x", ASK, "risky:system"),
        ("installer -pkg x.pkg -target /", ASK, "risky:system"),
        ("softwareupdate -i -a", ASK, "risky:system"),
        ("tccutil reset All", ASK, "risky:system"),
        ("visudo", ASK, "risky:system"),
        ("sudoedit /etc/hosts", ASK, "risky:system"),
        (
            "awk 'BEGIN{system(\"git push\")}'",
            ASK,
            "risky:interpreter",
        ),
        ("sed 'e git push' /dev/null", ASK, "risky:interpreter"),
        ("gsed 'e git push' /dev/null", ASK, "risky:interpreter"),
        ("sed -n 's/a/b/p' data.txt", ALLOW, "shellInScope"),
        (
            "sqlite3 :memory: '.shell git push'",
            ASK,
            "risky:interpreter",
        ),
        ("sqlite3 db.sqlite 'select 1'", ALLOW, "shellInScope"),
        ("tmux list-sessions", ALLOW, "shellInScope"),
        ("plutil -p Info.plist", ALLOW, "shellInScope"),
        // 10. writes outside scope through non-file-command tools.
        ("tar -C /tmp -xf a.tar", ASK, "outsideScope"),
        ("tar -xf a.tar -C /Users/me", ASK, "outsideScope"),
        ("tar -C /work/project -xf a.tar", ALLOW, "shellInScope"),
        ("unzip -d /tmp a.zip", ASK, "outsideScope"),
        ("7z x -o/tmp a.7z", ASK, "outsideScope"),
        ("patch -d /tmp -p1", ASK, "outsideScope"),
        ("perl -pi -e 's/a/b/' /tmp/x", ASK, "outsideScope"),
        ("ruby -i -pe 's/a/b/' /tmp/x", ASK, "outsideScope"),
        ("perl -pi -e 's/a/b/' src/main.rs", ALLOW, "shellInScope"),
        (
            "git clone https://example.com/r.git /tmp/r",
            ASK,
            "outsideScope",
        ),
        ("git clone https://example.com/r.git", ALLOW, "shellInScope"),
        (
            "git --work-tree=/tmp --git-dir=/tmp/g checkout -f",
            ASK,
            "outsideScope",
        ),
        ("vim -e -c 'wq' /tmp/x", ASK, "outsideScope"),
        ("ed /tmp/x", ASK, "outsideScope"),
        ("ex -sc '%s/a/b/|wq' /tmp/x", ASK, "outsideScope"),
        ("vim src/main.rs", ALLOW, "shellInScope"),
        // 11. renamed binaries.
        (
            "ln -s /usr/bin/curl c && ./c -d a=1 https://example.com",
            ASK,
            "unresolvedPath",
        ),
        (
            "cp ./tools/git g && ./g push origin main",
            ASK,
            "unresolvedPath",
        ),
        ("ln -s /usr/bin/ssh s && ./s host", ASK, "unresolvedPath"),
        // 14. rmdir / unlink of a protected root.
        ("rmdir /work/project", ASK, "risky:rm-root"),
        ("unlink /work/agents/Bot", ASK, "risky:rm-root"),
        ("rmdir /work/project/sub", ALLOW, "shellInScope"),
        // --- Fix round 3: same-class variants ---
        // 2. `git worktree` destinations are write targets.
        ("git worktree add /tmp/x", ASK, "outsideScope"),
        ("git worktree add ../wt", ASK, "outsideScope"),
        ("git worktree add -b feat /tmp/wt main", ASK, "outsideScope"),
        ("git worktree move wt /tmp/wt", ASK, "outsideScope"),
        ("git worktree remove /tmp/wt", ASK, "outsideScope"),
        ("git worktree add /work/project/wt-x", ALLOW, "shellInScope"),
        (
            "git worktree add -b feat /work/project/wt-y main",
            ALLOW,
            "shellInScope",
        ),
        ("git worktree list", ALLOW, "shellInScope"),
        // 3. awk input pipes run a command.
        (
            "awk 'BEGIN{\"git push\" | getline}'",
            ASK,
            "risky:interpreter",
        ),
        (
            "awk 'BEGIN{cmd = \"id\"; cmd |getline x}'",
            ASK,
            "risky:interpreter",
        ),
        ("awk -f prog.awk data.txt", ASK, "risky:interpreter"),
        ("awk '{print $1}' f", ALLOW, "shellInScope"),
        ("awk 'NR==1{getline line < \"f\"}' g", ALLOW, "shellInScope"),
        // 4. sed `e` after an address, and in -e and cluster forms.
        ("sed -e '1e git push'", ASK, "risky:interpreter"),
        ("sed '$e id'", ASK, "risky:interpreter"),
        ("sed '/x/e date' f", ASK, "risky:interpreter"),
        ("sed '1,3e id' f", ASK, "risky:interpreter"),
        ("sed '1~2 e id' f", ASK, "risky:interpreter"),
        ("sed '/a/,/b/!e id' f", ASK, "risky:interpreter"),
        ("sed -n -e 'p' -e '2e id' f", ASK, "risky:interpreter"),
        ("sed -ne '1e id' f", ASK, "risky:interpreter"),
        ("sed --expression='1e id' f", ASK, "risky:interpreter"),
        ("sed '1{e id\n}' f", ASK, "risky:interpreter"),
        ("sed 's/a/id/e' f", ASK, "risky:interpreter"),
        ("sed -f script.sed f", ASK, "risky:interpreter"),
        ("sed -e 's/e/E/' f", ALLOW, "shellInScope"),
        ("sed -n '1p' f", ALLOW, "shellInScope"),
        ("sed '/e/d' f", ALLOW, "shellInScope"),
        ("sed 's/\\/e/x/g' f", ALLOW, "shellInScope"),
        ("sed -e 'y/abc/xyz/' f", ALLOW, "shellInScope"),
        ("sed '1a text with e' f", ALLOW, "shellInScope"),
        ("sed -i '' '1e id' f", ASK, "risky:interpreter"),
        ("sed -i .bak '$e id' f", ASK, "risky:interpreter"),
        ("sed -i.bak '1e id' f", ASK, "risky:interpreter"),
        ("sed -i '' 's/a/b/' f", ALLOW, "shellInScope"),
        ("sed -E -n 's/(e)/\\1/p' f", ALLOW, "shellInScope"),
        // 5. npx scoped packages match the name and the scope.
        ("npx -y @scope/vercel@1 --prod", ASK, "risky:deploy"),
        ("npx -y @vercel/cli --prod", ASK, "risky:deploy"),
        ("npx -y @angular/cli build", ALLOW, "shellInScope"),
        ("npx -y @scope/npm@9 publish", ASK, "risky:publish"),
        ("npx -y @biomejs/biome check .", ALLOW, "shellInScope"),
        // 6. More send and system binaries.
        ("busybox ftpput host f", ASK, "risky:send"),
        ("ftpput host f", ASK, "risky:send"),
        ("ssh-copy-id host", ASK, "risky:system"),
        ("busybox ls", ALLOW, "shellInScope"),
        // 7. Unwrap sub-tokens fold case.
        ("bundle EXEC gem push x.gem", ASK, "risky:publish"),
        ("BUNDLE Exec gem push x.gem", ASK, "risky:publish"),
        ("BUSYBOX FTPPUT host f", ASK, "risky:send"),
        ("bundle EXEC rake test", ALLOW, "shellInScope"),
        // `cd` moves where relative paths land, and never narrows it.
        ("cd /work/project && rm -rf build", ALLOW, "shellInScope"),
        ("cd .. && rm -rf Bot", ASK, "risky:rm-root"),
        ("cd $X && rm -rf y", ASK, "unresolvedPath"),
        ("cd - && rm y", ASK, "unresolvedPath"),
        ("cd && rm -rf x", ASK, "outsideScope"),
        (
            "cd /work/project/sub && echo hi > out.txt",
            ALLOW,
            "shellInScope",
        ),
        // --- Fix round 4: git's own base directory ---
        // `-C <dir>` moves where git's relative paths land.
        ("git -C /tmp worktree add wt", ASK, "outsideScope"),
        ("git -C /tmp clone https://x/r.git r", ASK, "outsideScope"),
        ("git -C /tmp init repo", ASK, "outsideScope"),
        (
            "git --work-tree=wt -C /tmp checkout -f",
            ASK,
            "outsideScope",
        ),
        (
            "git -C /tmp --work-tree=wt checkout -f",
            ASK,
            "outsideScope",
        ),
        ("git --git-dir g -C /tmp status", ASK, "outsideScope"),
        (
            "git --git-dir g -C /work/project status",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project worktree add wt",
            ALLOW,
            "shellInScope",
        ),
        ("git -C ../project worktree add wt", ASK, "outsideScope"),
        (
            "git -C /work/project -C sub init repo",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project -C ../agents/Bot commit -am x",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project -C ../../tmp commit -am x",
            ASK,
            "outsideScope",
        ),
        ("git -C \"$D\" init repo", ASK, "unresolvedPath"),
        ("git -C /t* init repo", ASK, "unresolvedPath"),
        ("git -C '' init repo", ASK, "unresolvedPath"),
        ("git -C", ASK, "unresolvedPath"),
        ("git -C ~/src commit -am x", ASK, "outsideScope"),
        ("git -C ~/.ssh status", ASK, "credentialPath"),
        (
            "git -C /work/project/.claude commit -am x",
            ASK,
            "risky:agent-config",
        ),
        ("git -C /work/project/.claude status", ALLOW, "shellInScope"),
        (
            "git -C /work/project worktree add /tmp/wt",
            ASK,
            "outsideScope",
        ),
        // Reads of another repository are outside reads; a harness folder in
        // scope still separates git's reads (allowed) from its writes (asked).
        ("git -C /tmp status", ASK, "outsideScope"),
        ("git -C /tmp log --oneline -5", ASK, "outsideScope"),
        ("git -C ~/other-repo show HEAD", ASK, "outsideScope"),
        (
            "git -C /work/project/.claude commit -am x",
            ASK,
            "risky:agent-config",
        ),
        ("git -C /work/project/.claude status", ALLOW, "shellInScope"),
        (
            "git -C /work/project/.claude log --oneline -5",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude diff HEAD~1",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude show HEAD",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude rev-parse HEAD",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude ls-files",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude config --get user.email",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude config user.email a@b.c",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude --version",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude fetch origin",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude checkout main",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude add .",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude stash",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude stash list",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude worktree list",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude remote -v",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude remote add o https://x/r.git",
            ASK,
            "risky:agent-config",
        ),
        ("git -C /work/project/.claude branch", ALLOW, "shellInScope"),
        (
            "git -C /work/project/.claude branch -avv",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude branch --show-current",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude branch feat",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude branch -m old new",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude tag -l 'v*'",
            ALLOW,
            "shellInScope",
        ),
        (
            "git -C /work/project/.claude tag v1",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude frobnicate",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude clone https://x/r.git",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project/.claude init",
            ASK,
            "risky:agent-config",
        ),
        (
            "git -C /work/project clone https://x/r.git",
            ALLOW,
            "shellInScope",
        ),
        ("git -C /work/project init", ALLOW, "shellInScope"),
        ("git -C /work/project commit -am x", ALLOW, "shellInScope"),
        ("cd /tmp && git commit -am x", ASK, "outsideScope"),
        ("cd /tmp && git status", ASK, "cwdOutsideScope"),
        (
            "cd /tmp && git -C /work/project commit -am x",
            ASK,
            "cwdOutsideScope",
        ),
        ("cd $X && git commit -am x", ASK, "unresolvedPath"),
        ("cd $X && git status", ASK, "cwdOutsideScope"),
        // Pure reads still write where `--output` points.
        ("git diff --output=/tmp/d.patch", ASK, "outsideScope"),
        ("git -C /tmp diff --output d.patch", ASK, "outsideScope"),
        ("git diff --output=d.patch", ALLOW, "shellInScope"),
        ("git log --output-indicator-new=+", ALLOW, "shellInScope"),
        // Clone options with values do not shift the destination.
        (
            "git clone -b main https://x/r.git /tmp/r",
            ASK,
            "outsideScope",
        ),
        (
            "git clone --depth 1 https://x/r.git /work/project/r",
            ALLOW,
            "shellInScope",
        ),
        (
            "git init --separate-git-dir /tmp/g repo",
            ASK,
            "outsideScope",
        ),
        ("git init -b main repo", ALLOW, "shellInScope"),
        ("git format-patch -o /tmp/p HEAD~1", ASK, "outsideScope"),
        ("git format-patch -o patches HEAD~1", ALLOW, "shellInScope"),
        ("git archive -o /tmp/a.tar HEAD", ASK, "outsideScope"),
        ("git archive -o a.tar HEAD", ALLOW, "shellInScope"),
        ("git bundle create /tmp/b.bundle main", ASK, "outsideScope"),
        ("git bundle create b.bundle main", ALLOW, "shellInScope"),
        // --- Final review: the shell's working directory persists between commands ---
        // Claude's Bash tool keeps a `cd` for the next command, so a command may not end with
        // its working directory outside scope or unknown.
        ("cd ~/Documents", ASK, "cwdOutsideScope"),
        ("cd $HOME/Documents", ASK, "cwdOutsideScope"),
        ("cd /tmp", ASK, "cwdOutsideScope"),
        ("cd", ASK, "cwdOutsideScope"),
        ("cd ~/Documents; ls", ASK, "cwdOutsideScope"),
        ("cd /work/project && cargo test", ALLOW, "shellInScope"),
        ("cd ./sub", ALLOW, "shellInScope"),
        ("cd /work/project/sub && ls", ALLOW, "shellInScope"),
        ("pushd /tmp", ASK, "cwdOutsideScope"),
        ("pushd ./sub", ALLOW, "shellInScope"),
        ("popd", ASK, "cwdOutsideScope"),
        ("cd -", ASK, "cwdOutsideScope"),
        ("cd .. && ls", ASK, "cwdOutsideScope"),
        ("builtin cd /tmp", ASK, "cwdOutsideScope"),
        ("command cd /tmp", ASK, "cwdOutsideScope"),
        ("cd -P /tmp", ASK, "cwdOutsideScope"),
        ("cd -- /tmp", ASK, "cwdOutsideScope"),
        // zsh's two-argument `cd old new` substitutes inside the current path.
        ("cd Bot tmp", ASK, "cwdOutsideScope"),
        ("cd ~root", ASK, "cwdOutsideScope"),
        // `+N` and `-N` pick a directory off the stack.
        ("pushd +1", ASK, "cwdOutsideScope"),
        ("cd +1", ASK, "cwdOutsideScope"),
        ("cd -1", ASK, "cwdOutsideScope"),
        ("(cd /tmp)", ASK, "unparseable"),
        // --- Ruling 34 A: directory changes the policy did not model ---
        // zsh's `chdir` is `cd`.
        ("chdir /tmp", ASK, "cwdOutsideScope"),
        ("chdir ~ && rm -rf Documents", ASK, "outsideScope"),
        ("chdir", ASK, "cwdOutsideScope"),
        ("chdir ./sub", ALLOW, "shellInScope"),
        // A bare name may resolve through the owner's `CDPATH` or `cdablevars`.
        ("cd sub", ASK, "cwdOutsideScope"),
        ("cd tmp && rm -rf stuff", ASK, "unresolvedPath"),
        ("cd sub/dir", ASK, "cwdOutsideScope"),
        ("pushd sub", ASK, "cwdOutsideScope"),
        ("chdir sub", ASK, "cwdOutsideScope"),
        ("cd ./sub && rm -rf build", ALLOW, "shellInScope"),
        ("cd .", ALLOW, "shellInScope"),
        ("cd ../Bot/sub", ALLOW, "shellInScope"),
        // Anything that rewrites how `cd` or `~` resolve.
        ("cdpath=/; cd tmp", ASK, "unresolvedPath"),
        ("CDPATH+=/; cd tmp", ASK, "unresolvedPath"),
        ("set -A cdpath /; cd tmp", ASK, "unresolvedPath"),
        ("setopt cdablevars; X=/tmp; cd X", ASK, "unresolvedPath"),
        ("cdpath=/; cd tmp && rm -rf stuff", ASK, "unresolvedPath"),
        ("cdpath=(/); cd tmp", ASK, "unparseable"),
        ("export CDPATH=/; cd ./tmp", ASK, "unresolvedPath"),
        ("typeset -x cdpath=/", ASK, "unresolvedPath"),
        ("declare -n ref=CDPATH", ASK, "unresolvedPath"),
        ("read -A cdpath < dirs.txt", ASK, "unresolvedPath"),
        ("cdpath[1]=/", ASK, "unresolvedPath"),
        ("HOME+=/x; cd; rm -rf y", ASK, "unresolvedPath"),
        ("setopt CDABLE_VARS", ASK, "unresolvedPath"),
        ("setopt C_dAbLe_VaRs", ASK, "unresolvedPath"),
        ("set -o autocd", ASK, "unresolvedPath"),
        ("setopt auto_cd", ASK, "unresolvedPath"),
        ("unsetopt nocdablevars", ASK, "unresolvedPath"),
        ("shopt -s cdable_vars", ASK, "unresolvedPath"),
        ("options[autocd]=on", ASK, "unresolvedPath"),
        // zsh's single-letter spellings: `-T` is cdablevars, `-J` autocd.
        ("set -T; X=/tmp; cd X", ASK, "unresolvedPath"),
        ("setopt -J", ASK, "unresolvedPath"),
        // `-m` sets every option matching a pattern.
        ("setopt -m 'cd*'", ASK, "unresolvedPath"),
        ("set -euo pipefail", ALLOW, "shellInScope"),
        // Candidates accumulate across `&&` too: a `||` list continued across a newline or a
        // `#` comment can skip the `cd`. Accepted false ask: a `cd ./sub && … && cd ..` round trip.
        ("cd ./sub && make && cd ..", ASK, "cwdOutsideScope"),
        ("cd ./sub && cd ../sub2 && cd ..", ASK, "cwdOutsideScope"),
        ("true ||\ncd ./sub && rm -rf ../Other", ASK, "outsideScope"),
        (
            "true || # c\ncd ./sub && rm -rf ../Other",
            ASK,
            "outsideScope",
        ),
        ("cd ./sub; cd ..", ASK, "cwdOutsideScope"),
        ("cd ./sub || cd ..", ASK, "cwdOutsideScope"),
        ("true || cd ./sub && cd ..", ASK, "cwdOutsideScope"),
        ("! cd ./sub && cd ..", ASK, "cwdOutsideScope"),
        ("nice cd ./sub && cd ..", ASK, "cwdOutsideScope"),
        ("ls | cd ./sub && cd ..", ASK, "cwdOutsideScope"),
        ("cd ./sub | cat && cd ..", ASK, "cwdOutsideScope"),
        // A loop may repeat a `cd` from wherever the last pass ended.
        (
            "for i in 1 2; do cd ../../project; done; rm -rf x",
            ASK,
            "unresolvedPath",
        ),
        ("while true; do cd ./sub; done", ASK, "cwdOutsideScope"),
        ("for f in a b; do rm -rf build; done", ALLOW, "shellInScope"),
        // --- Final review: git options that run a command ---
        ("git grep -O foo", ASK, "risky:interpreter"),
        ("git grep -Ovim foo", ASK, "risky:interpreter"),
        ("git grep -O vim foo", ASK, "risky:interpreter"),
        ("git grep -nO foo", ASK, "risky:interpreter"),
        (
            "git grep --open-files-in-pager foo",
            ASK,
            "risky:interpreter",
        ),
        (
            "git grep --open-files-in-pager=vim foo",
            ASK,
            "risky:interpreter",
        ),
        ("git -C /work/project grep -O foo", ASK, "risky:interpreter"),
        ("git grep foo", ALLOW, "shellInScope"),
        ("git grep -n -e FOO", ALLOW, "shellInScope"),
        ("git grep -eFOO", ALLOW, "shellInScope"),
        ("git difftool -x vimdiff", ASK, "risky:interpreter"),
        ("git difftool -xvimdiff", ASK, "risky:interpreter"),
        ("git difftool -yx vimdiff", ASK, "risky:interpreter"),
        ("git difftool --extcmd vimdiff", ASK, "risky:interpreter"),
        ("git difftool --extcmd=vimdiff", ASK, "risky:interpreter"),
        ("git mergetool -x vimdiff", ASK, "risky:interpreter"),
        ("git mergetool --extcmd=vimdiff", ASK, "risky:interpreter"),
        ("git difftool --tool=vimdiff", ALLOW, "shellInScope"),
        ("git difftool -y -t vimdiff", ALLOW, "shellInScope"),
        ("git difftool -txxdiff", ALLOW, "shellInScope"),
        // --- Ruling 34 B: git resolves any unambiguous long-option prefix ---
        ("git grep --open=touch M foo", ASK, "risky:interpreter"),
        ("git grep --ope=vim foo", ASK, "risky:interpreter"),
        ("git grep --op=sh -c id foo", ASK, "risky:interpreter"),
        ("git rebase --exe=touch M HEAD~1", ASK, "risky:interpreter"),
        ("git rebase --exec=touch M HEAD~1", ASK, "risky:interpreter"),
        (
            "git difftool -y --extc=touch M HEAD~1",
            ASK,
            "risky:interpreter",
        ),
        ("git mergetool --extc=x", ASK, "risky:interpreter"),
        (
            "git clone --up=touch M; git-upload-pack . r",
            ASK,
            "risky:interpreter",
        ),
        ("git ls-remote --exec=touch M .", ASK, "risky:interpreter"),
        ("git fetch --upl=touch M origin", ASK, "risky:interpreter"),
        (
            "git clone --conf=core.x=y https://x/r.git r",
            ASK,
            "risky:interpreter",
        ),
        (
            "git clone --templ=/etc https://x/r.git r",
            ASK,
            "risky:agent-config",
        ),
        ("git init --templ=/etc repo", ASK, "risky:agent-config"),
        // Path-carrying long options resolve the same way.
        ("git diff --outp=/tmp/d.patch", ASK, "outsideScope"),
        ("git --work-tr=/tmp status", ASK, "unparseable"),
        (
            "git clone --sep=/tmp/g https://x/r.git r",
            ASK,
            "outsideScope",
        ),
        // Benign prefixes of nothing risky stay allowed.
        ("git grep --count foo", ALLOW, "shellInScope"),
        ("git grep --coun foo", ALLOW, "shellInScope"),
        ("git log --oneline", ALLOW, "shellInScope"),
        ("git log --onel", ALLOW, "shellInScope"),
        ("git status --porcelain", ALLOW, "shellInScope"),
        ("git status --por", ALLOW, "shellInScope"),
        ("git log --output-indicator-new=+", ALLOW, "shellInScope"),
        ("git ls-files --exclude=*.log", ALLOW, "shellInScope"),
    ];

    #[allow(clippy::too_many_lines)] // A data table.
    fn facts_table() -> Vec<(ApprovalFacts, bool, &'static str)> {
        vec![
            // The task brief's minimum rows.
            (read(&["/etc/hosts"]), ASK, "outsideScope"),
            (read(&["/Users/me/.ssh/id_rsa"]), ASK, "credentialPath"),
            (search(&[]), ALLOW, "search"),
            (write(&["/work/project/src/main.rs"]), ALLOW, "inScopeWrite"),
            (write(&["/work/project/.env"]), ASK, "credentialPath"),
            (write(&["/tmp/x"]), ASK, "outsideScope"),
            (write(&["/work/project/../other/x"]), ASK, "outsideScope"),
            (delete(&["/work/agents/Bot/old.txt"]), ALLOW, "inScopeWrite"),
            (delete(&["/work/elsewhere"]), ASK, "outsideScope"),
            (ApprovalFacts::WebSearch {}, ASK, "webSearch"),
            (fetch("GET", "https://example.com"), ASK, "webFetch"),
            (fetch("POST", "https://example.com"), ASK, "webMethod"),
            (mcp("github", "get_issue"), ASK, "mcpAction"),
            (mcp("github", "list_pulls"), ASK, "mcpAction"),
            (mcp("github", "create_issue"), ASK, "mcpAction"),
            (mcp("gmail", "send_message"), ASK, "mcpAction"),
            (mcp("slack", "post_message"), ASK, "mcpAction"),
            (
                ApprovalFacts::Internal {
                    tool: "TodoWrite".to_owned(),
                },
                ALLOW,
                "internal",
            ),
            (
                ApprovalFacts::Unknown {
                    tool: "AskUserQuestion".to_owned(),
                },
                ASK,
                "unknownTool",
            ),
            // Reads and searches.
            (read(&["/work/project/src/lib.rs"]), ALLOW, "read"),
            (read(&[]), ALLOW, "read"),
            (read(&["config/.env.local"]), ASK, "credentialPath"),
            (read(&["~/.ssh/config"]), ASK, "credentialPath"),
            (
                read(&["/etc/hosts", "/p/.env.local"]),
                ASK,
                "credentialPath",
            ),
            (read(&["/P/.SSH/known_hosts"]), ASK, "credentialPath"),
            (search(&["/Users/me/.gnupg"]), ASK, "credentialPath"),
            (search(&["/work/project", "/opt"]), ASK, "outsideScope"),
            (search(&["/work/project", "src"]), ALLOW, "search"),
            (read(&["~/Documents/taxes.pdf"]), ASK, "outsideScope"),
            // Writes and deletes.
            (write(&["notes/todo.md"]), ALLOW, "inScopeWrite"),
            (write(&["~/notes.txt"]), ASK, "outsideScope"),
            (write(&["/work/projectile/x"]), ASK, "outsideScope"),
            (write(&["/work/agents/Bot/../Other/x"]), ASK, "outsideScope"),
            (write(&["/work/project/a", "/tmp/b"]), ASK, "outsideScope"),
            (
                write(&["/work/project/secrets/token.txt"]),
                ASK,
                "credentialPath",
            ),
            (write(&["/work/project/id_ed25519"]), ASK, "credentialPath"),
            (write(&[]), ASK, "unresolvedPath"),
            (write(&[""]), ASK, "unresolvedPath"),
            (delete(&["/work/project"]), ASK, "risky:rm-root"),
            (delete(&["/work/agents/Bot/"]), ASK, "risky:rm-root"),
            (delete(&["/"]), ASK, "risky:rm-root"),
            (delete(&["~"]), ASK, "risky:rm-root"),
            (delete(&[]), ASK, "unresolvedPath"),
            // Harness configuration is never an agent's to write.
            (
                write(&["/work/agents/Bot/.claude/settings.local.json"]),
                ASK,
                "risky:agent-config",
            ),
            (
                write(&["/work/project/.mcp.json"]),
                ASK,
                "risky:agent-config",
            ),
            (
                write(&["/work/project/.grok/config.toml"]),
                ASK,
                "risky:agent-config",
            ),
            (
                write(&["/Users/me/.claude/settings.json"]),
                ASK,
                "credentialPath",
            ),
            (delete(&["/work/project/.codex"]), ASK, "risky:agent-config"),
            (
                write(&["/work/project/.claude/.env"]),
                ASK,
                "credentialPath",
            ),
            (
                write(&["/work/project/claude/notes.md"]),
                ALLOW,
                "inScopeWrite",
            ),
            (
                write(&["/work/project/docs/mcp.json"]),
                ALLOW,
                "inScopeWrite",
            ),
            (
                read(&["/work/agents/Bot/.claude/settings.json"]),
                ALLOW,
                "read",
            ),
            (read(&["/work/project/.mcp.json"]), ALLOW, "read"),
            // Credential stores the shared tables do not name.
            (read(&["~/.config/gh/hosts.yml"]), ASK, "credentialPath"),
            (read(&["/Users/me/.config/gh"]), ASK, "credentialPath"),
            (
                read(&["~/.claude/.credentials.json"]),
                ASK,
                "credentialPath",
            ),
            (read(&["~/.codex/auth.json"]), ASK, "credentialPath"),
            (
                read(&["~/.grok/sessions/s1/chat_history.jsonl"]),
                ASK,
                "credentialPath",
            ),
            (
                read(&["/Users/me/Library/Application Support/Interface/phone-connection.v1.json"]),
                ASK,
                "credentialPath",
            ),
            (
                read(&["/Users/me/Library/Application Support/Interface Dev/state.json"]),
                ASK,
                "credentialPath",
            ),
            (
                read(&["/work/project/thread-secrets/a/TOKEN"]),
                ASK,
                "credentialPath",
            ),
            (
                read(&["/Users/me/Library/Keychains/login.keychain-db"]),
                ASK,
                "credentialPath",
            ),
            (
                read(&["/USERS/ME/library/KEYCHAINS/login.keychain-db"]),
                ASK,
                "credentialPath",
            ),
            (search(&["/Users/me/.config/gcloud"]), ASK, "credentialPath"),
            (search(&["~/.password-store"]), ASK, "credentialPath"),
            (
                read(&["/Users/me/.local/share/keyrings/login.keyring"]),
                ASK,
                "credentialPath",
            ),
            (read(&["/srv/tools/GH/hosts.yml"]), ASK, "credentialPath"),
            (
                write(&["/work/project/gh/hosts.yml"]),
                ASK,
                "credentialPath",
            ),
            (read(&["/Users/me/.config/git/config"]), ASK, "outsideScope"),
            (
                read(&["/Users/me/.config/ghostty/config"]),
                ASK,
                "outsideScope",
            ),
            (read(&["/work/project/hosts.yml"]), ALLOW, "read"),
            (
                read(&["/Users/other/.config/gh/config.yml"]),
                ASK,
                "outsideScope",
            ),
            // Git hooks and repository configuration.
            (
                write(&["/work/project/.git/hooks/pre-commit"]),
                ASK,
                "risky:agent-config",
            ),
            (
                write(&["/work/project/.git/config"]),
                ASK,
                "risky:agent-config",
            ),
            (
                delete(&["/work/project/.git/hooks"]),
                ASK,
                "risky:agent-config",
            ),
            (delete(&["/work/project/.git"]), ASK, "risky:agent-config"),
            (
                write(&["/work/project/.git/info/exclude"]),
                ALLOW,
                "inScopeWrite",
            ),
            (
                write(&["/work/project/.github/workflows/ci.yml"]),
                ALLOW,
                "inScopeWrite",
            ),
            (read(&["/work/project/.git/config"]), ALLOW, "read"),
            // Web.
            (fetch("get", "https://example.com"), ASK, "webFetch"),
            (fetch("HEAD", "https://example.com"), ASK, "webMethod"),
            (fetch("DELETE", "https://example.com"), ASK, "webMethod"),
            (fetch("", "https://example.com"), ASK, "webMethod"),
            // MCP.
            (mcp("github", "mcp__github__search_code"), ASK, "mcpAction"),
            (mcp("github", "github__list_repos"), ASK, "mcpAction"),
            (mcp("github", "mcp__other__delete_repo"), ASK, "mcpAction"),
            (mcp("linear", "getIssue"), ASK, "mcpAction"),
            (mcp("linear", "GetIssue"), ASK, "mcpAction"),
            (mcp("x", "list-items"), ASK, "mcpAction"),
            (mcp("x", "status"), ASK, "mcpAction"),
            (mcp("x", "describe_table"), ASK, "mcpAction"),
            (mcp("x", "getaway"), ASK, "mcpAction"),
            (mcp("x", "reader"), ASK, "mcpAction"),
            (mcp("x", "delete_issue"), ASK, "mcpAction"),
            (mcp("x", "merge_pull_request"), ASK, "mcpAction"),
            (mcp("x", "update_page"), ASK, "mcpAction"),
            (mcp("x", ""), ASK, "mcpAction"),
            // Read-verb tightening: a later mutating word or a secret word asks.
            (mcp("github", "get_issue"), ASK, "mcpAction"),
            (mcp("x", "getSecret"), ASK, "mcpAction"),
            (mcp("x", "get_and_post"), ASK, "mcpAction"),
            (mcp("x", "searchReplace"), ASK, "mcpAction"),
            (mcp("x", "list_files"), ASK, "mcpAction"),
            (mcp("x", "fetch_and_delete"), ASK, "mcpAction"),
            (mcp("x", "read_and_send"), ASK, "mcpAction"),
            (mcp("x", "listCreate"), ASK, "mcpAction"),
            (mcp("x", "get_api_token"), ASK, "mcpAction"),
            (mcp("x", "readPassword"), ASK, "mcpAction"),
            (mcp("x", "get_ssh_key"), ASK, "mcpAction"),
            (mcp("x", "getUserById"), ASK, "mcpAction"),
            (mcp("x", "list_deployments"), ASK, "mcpAction"),
            // Plural and compound credential words.
            (mcp("x", "list_secrets"), ASK, "mcpAction"),
            (mcp("x", "listSecrets"), ASK, "mcpAction"),
            (mcp("x", "getSecrets"), ASK, "mcpAction"),
            (mcp("x", "get_tokens"), ASK, "mcpAction"),
            (mcp("x", "list_keys"), ASK, "mcpAction"),
            (mcp("x", "describe_secrets"), ASK, "mcpAction"),
            (mcp("x", "list_passwords"), ASK, "mcpAction"),
            (mcp("x", "get_credentials"), ASK, "mcpAction"),
            (mcp("x", "get_apikey"), ASK, "mcpAction"),
            (mcp("x", "get_api_key"), ASK, "mcpAction"),
            (mcp("x", "listApiKeys"), ASK, "mcpAction"),
            (mcp("x", "list_keyboards"), ASK, "mcpAction"),
            (mcp("x", "get_monkey"), ASK, "mcpAction"),
            (mcp("x", "list_statuses"), ASK, "mcpAction"),
            (
                ApprovalFacts::Unknown {
                    tool: "Bash".to_owned(),
                },
                ASK,
                "unknownTool",
            ),
            // A `cwd` override moves where relative shell paths land.
            (shell_in("rm -rf x", "/tmp"), ASK, "outsideScope"),
            (
                shell_in("rm -rf build", "/work/project"),
                ALLOW,
                "shellInScope",
            ),
            (shell_in("rm -rf .", "/work/project"), ASK, "risky:rm-root"),
            (shell_in("echo hi > out.txt", "sub"), ALLOW, "shellInScope"),
            // Fix round 4: git writes in its working directory.
            (shell_in("git commit -am x", "/tmp"), ASK, "outsideScope"),
            (shell_in("git status", "/tmp"), ALLOW, "shellInScope"),
            (shell_in("git init repo", "/tmp"), ASK, "outsideScope"),
            // Final review: the provider-reported start directory is not a `cd`; a `cd` away is.
            (shell_in("ls", "/tmp"), ALLOW, "shellInScope"),
            (shell_in("cd /tmp", "/work/project"), ASK, "cwdOutsideScope"),
            (shell_in("cd ./sub", "/work/project"), ALLOW, "shellInScope"),
            (shell_in("cd sub", "/work/project"), ASK, "cwdOutsideScope"),
            // Every candidate stays possible after a `cd`, including where the command started.
            (
                shell_in("cd /work/project && rm -rf build", "/tmp"),
                ASK,
                "outsideScope",
            ),
            (
                shell_in("true || cd /work/project && rm -rf build", "/tmp"),
                ASK,
                "outsideScope",
            ),
            (
                shell_in("! cd /work/project && rm -rf build", "/tmp"),
                ASK,
                "outsideScope",
            ),
            (
                shell_in("env cd /work/project && rm -rf build", "/tmp"),
                ASK,
                "outsideScope",
            ),
        ]
    }

    fn assert_decision(facts: &ApprovalFacts, scope: &AgentScope, allow: bool, reason: &str) {
        let decision = classify(facts, scope);
        assert_eq!(
            (decision.is_allow(), decision.reason()),
            (allow, reason),
            "{facts:?}"
        );
    }

    #[test]
    fn web_fetch_asks_the_owner_first_whatever_the_method_or_task() {
        // The owner chose ask-first on 2026-10-14: a GET can carry anything an
        // agent read in scope, so no fetch is automatic. Since 2026-11-21 a
        // search asks too, unless the owner switched searching on.
        let scope = scope();
        for (method, reason) in [
            ("GET", "webFetch"),
            ("get", "webFetch"),
            ("POST", "webMethod"),
            ("", "webMethod"),
        ] {
            let facts = fetch(method, "https://example.com/?q=notes");
            assert_decision(&facts, &scope, ASK, reason);
            for allow_opaque in [false, true] {
                let decision = classify_task(&facts, &scope, allow_opaque);
                assert_eq!(
                    (decision.is_allow(), decision.reason()),
                    (ASK, reason),
                    "{method} with allow_opaque {allow_opaque}"
                );
            }
        }
        assert_decision(&ApprovalFacts::WebSearch {}, &scope, ASK, "webSearch");
    }

    #[test]
    fn a_teammates_web_search_asks_unless_its_owner_switched_searching_on() {
        let scope = scope();
        let search = ApprovalFacts::WebSearch {};
        for allow_opaque in [false, true] {
            let off = classify_for_owner(
                &search,
                &scope,
                OwnerSwitches {
                    allow_opaque,
                    allow_web_search: false,
                },
            );
            assert_eq!((off.is_allow(), off.reason()), (ASK, "webSearch"));
            let on = classify_for_owner(
                &search,
                &scope,
                OwnerSwitches {
                    allow_opaque,
                    allow_web_search: true,
                },
            );
            assert_eq!((on.is_allow(), on.reason()), (ALLOW, "webSearchAllowed"));
            let decision = classify_task(&search, &scope, allow_opaque);
            assert_eq!((decision.is_allow(), decision.reason()), (ASK, "webSearch"));
        }
        // The switch does not depend on the agent's folders.
        let relative = AgentScope {
            cwd: PathBuf::from("work/project"),
            ..scope
        };
        let searching = OwnerSwitches {
            allow_opaque: false,
            allow_web_search: true,
        };
        assert!(classify_for_owner(&search, &relative, searching).is_allow());
    }

    #[test]
    fn the_web_search_switch_does_not_loosen_web_fetches_or_anything_else() {
        let scope = scope();
        let searching = OwnerSwitches {
            allow_opaque: false,
            allow_web_search: true,
        };
        let rows = facts_table();
        let shell_rows = SHELL_TABLE
            .iter()
            .map(|(command, allow, reason)| (shell(command), *allow, *reason));
        let mut checked = 0;
        for (facts, _, _) in rows
            .iter()
            .map(|(facts, allow, reason)| (facts.clone(), *allow, *reason))
            .chain(shell_rows)
        {
            if matches!(facts, ApprovalFacts::WebSearch {}) {
                continue;
            }
            assert_eq!(
                classify_for_owner(&facts, &scope, searching),
                classify_task(&facts, &scope, false),
                "{facts:?}"
            );
            checked += 1;
        }
        assert!(checked > 100);
        for method in ["GET", "POST"] {
            let fetch = fetch(method, "https://example.com/?q=secret");
            assert!(!classify_for_owner(&fetch, &scope, searching).is_allow());
        }
    }

    #[test]
    fn policy_table() {
        let scope = scope();
        let rows = facts_table();
        let shell_rows = SHELL_TABLE
            .iter()
            .map(|(command, allow, reason)| (shell(command), *allow, *reason));
        // Every mismatch is reported, not just the first.
        let mismatches: Vec<String> = rows
            .iter()
            .map(|(facts, allow, reason)| (facts.clone(), *allow, *reason))
            .chain(shell_rows)
            .filter_map(|(facts, allow, reason)| {
                let decision = classify(&facts, &scope);
                ((decision.is_allow(), decision.reason()) != (allow, reason)).then(|| {
                    format!(
                        "{facts:?}: expected ({allow}, {reason:?}), got ({}, {:?})",
                        decision.is_allow(),
                        decision.reason()
                    )
                })
            })
            .collect();
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
        assert!(rows.len() + SHELL_TABLE.len() > 100);
        // Strict task mode reads the same short form.
        let strict = classify_task(&shell("if [[ -n x ]] git push origin main"), &scope, false);
        assert_eq!((strict.is_allow(), strict.reason()), (ASK, "unparseable"));
    }

    #[test]
    fn decision_accessors() {
        let allow = PolicyDecision::Allow {
            reason: "read".to_owned(),
        };
        let ask = PolicyDecision::AskOwner {
            reason: "unknownTool".to_owned(),
        };
        assert!(allow.is_allow());
        assert!(!ask.is_allow());
        assert_eq!((allow.reason(), ask.reason()), ("read", "unknownTool"));
    }

    #[test]
    fn approval_facts_wire_shape() {
        let facts = ApprovalFacts::Shell {
            command: "ls".to_owned(),
            cwd: None,
        };
        assert_eq!(
            serde_json::to_string(&facts).expect("serialize"),
            r#"{"kind":"shell","command":"ls","cwd":null}"#
        );
        let parsed: ApprovalFacts =
            serde_json::from_str(r#"{"kind":"webFetch","url":"https://x.dev","method":"GET"}"#)
                .expect("parse");
        assert_eq!(parsed, fetch("GET", "https://x.dev"));
        assert!(
            serde_json::from_str::<ApprovalFacts>(r#"{"kind":"webSearch","extra":1}"#).is_err()
        );
    }

    #[test]
    fn a_relative_scope_fails_closed() {
        let relative = AgentScope {
            roots: vec![PathBuf::from("/work/project")],
            cwd: PathBuf::from("work/project"),
            home_dir: PathBuf::from("/Users/me"),
        };
        assert_decision(&read(&["/etc/hosts"]), &relative, ASK, "unresolvedPath");
        assert_decision(&shell("ls"), &relative, ASK, "unresolvedPath");
        assert_decision(&ApprovalFacts::WebSearch {}, &relative, ASK, "webSearch");
        let homeless = AgentScope {
            home_dir: PathBuf::from("me"),
            ..scope()
        };
        assert_decision(
            &write(&["/work/project/a"]),
            &homeless,
            ASK,
            "unresolvedPath",
        );
    }

    #[test]
    fn globs_and_brace_groups_are_expanded_before_the_credential_check() {
        let tree = Tree::new();
        fs::create_dir_all(tree.base.join("root/config")).expect("config");
        fs::create_dir_all(tree.base.join("root/src")).expect("src");
        fs::create_dir_all(tree.base.join("home/.ssh")).expect("home ssh");
        for file in [
            "root/.env.local",
            "root/config/secrets.json",
            "root/src/a.rs",
            "home/.ssh/id_rsa",
        ] {
            fs::write(tree.base.join(file), "x").expect("file");
        }
        let scope = tree.scope();
        for command in [
            "cat .env*",
            "cat config/*",
            "cat conf?g/s[a-z]*.json",
            "cat */secrets.json",
            "cat .e{n,x}v.local",
            "cat ~/.s*/id_rsa",
            "wc -l < .env*",
        ] {
            assert_decision(&shell(command), &scope, ASK, "credentialPath");
        }
        for command in [
            "cat src/*.rs",
            "cat *",
            "grep -n 'foo.*' src/a.rs",
            "jq '{name,version}' src/a.rs",
        ] {
            assert_decision(&shell(command), &scope, ALLOW, "shellInScope");
        }
    }

    #[test]
    fn wildcard_matching() {
        use super::wildcard_matches;
        let matches = |pattern: &str, name: &str| {
            let pattern: Vec<char> = pattern.chars().collect();
            let name: Vec<char> = name.chars().collect();
            wildcard_matches(&pattern, &name)
        };
        assert!(matches("*.rs", "main.rs"));
        assert!(matches("a*b*c", "axxbyyc"));
        assert!(matches("*", ""));
        assert!(matches("s[a-z]*.json", "secrets.json"));
        assert!(matches("[]]x", "]x"));
        assert!(matches("[", "["));
        assert!(matches("a[", "a["));
        assert!(!matches("*.rs", "main.rsx"));
        assert!(!matches("?", ""));
        assert!(!matches("a*b", "acd"));
    }

    struct Tree {
        _dir: tempfile::TempDir,
        base: PathBuf,
    }

    impl Tree {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let base = dir.path().to_path_buf();
            fs::create_dir_all(base.join("root/real")).expect("root");
            fs::create_dir_all(base.join("outside/dir")).expect("outside");
            fs::create_dir_all(base.join("outside/.ssh")).expect("ssh");
            fs::write(base.join("outside/.ssh/id_rsa"), "key").expect("key");
            Self { _dir: dir, base }
        }

        fn path(&self, relative: &str) -> String {
            self.base.join(relative).to_string_lossy().into_owned()
        }

        fn link(&self, target: impl AsRef<Path>, name: &str) {
            symlink(target, self.base.join(name)).expect("symlink");
        }

        fn scope(&self) -> AgentScope {
            AgentScope {
                roots: vec![self.base.join("root")],
                cwd: self.base.join("root"),
                home_dir: self.base.join("home"),
            }
        }
    }

    #[test]
    fn symlink_inside_a_root_that_points_outside_is_outside_scope() {
        let tree = Tree::new();
        tree.link(tree.base.join("outside"), "root/link");
        let scope = tree.scope();
        assert_decision(
            &write(&[&tree.path("root/real/a.txt")]),
            &scope,
            ALLOW,
            "inScopeWrite",
        );
        assert_decision(
            &write(&[&tree.path("root/link/a.txt")]),
            &scope,
            ASK,
            "outsideScope",
        );
        assert_decision(
            &delete(&[&tree.path("root/link")]),
            &scope,
            ASK,
            "outsideScope",
        );
        assert_decision(
            &write(&["link/new/deeper.txt"]),
            &scope,
            ASK,
            "outsideScope",
        );
        assert_decision(
            &shell("echo hi > link/out.txt"),
            &scope,
            ASK,
            "outsideScope",
        );
        assert_decision(&shell("rm -rf link/dir"), &scope, ASK, "outsideScope");
        assert_decision(
            &shell("echo hi > real/out.txt"),
            &scope,
            ALLOW,
            "shellInScope",
        );
    }

    #[test]
    fn a_cd_through_a_link_that_leaves_scope_asks() {
        let tree = Tree::new();
        tree.link(tree.base.join("outside"), "root/link");
        let scope = tree.scope();
        assert_decision(&shell("cd link"), &scope, ASK, "cwdOutsideScope");
        assert_decision(&shell("cd link/dir && ls"), &scope, ASK, "cwdOutsideScope");
        assert_decision(
            &shell(&format!("cd {}", tree.path("root/link"))),
            &scope,
            ASK,
            "cwdOutsideScope",
        );
        assert_decision(&shell("cd ./real"), &scope, ALLOW, "shellInScope");
        assert_decision(&shell("cd real"), &scope, ASK, "cwdOutsideScope");
        assert_decision(
            &shell(&format!("cd {} && ls", tree.path("root/real"))),
            &scope,
            ALLOW,
            "shellInScope",
        );
    }

    #[test]
    fn parent_components_follow_the_real_directory_not_the_link_text() {
        let tree = Tree::new();
        tree.link(tree.base.join("outside/dir"), "root/deep");
        let scope = tree.scope();
        // The kernel resolves `deep/..` to `outside`, never back to `root`.
        assert_decision(
            &write(&[&tree.path("root/deep/../x")]),
            &scope,
            ASK,
            "outsideScope",
        );
        assert_decision(&shell("echo hi > deep/../x"), &scope, ASK, "outsideScope");
    }

    #[test]
    fn relative_dangling_and_looping_links_fail_closed() {
        let tree = Tree::new();
        tree.link("../outside", "root/relative");
        tree.link(tree.base.join("outside/not-yet"), "root/dangling");
        tree.link(tree.base.join("root/loop"), "root/loop");
        let scope = tree.scope();
        assert_decision(
            &write(&[&tree.path("root/relative/x")]),
            &scope,
            ASK,
            "outsideScope",
        );
        assert_decision(
            &write(&[&tree.path("root/dangling")]),
            &scope,
            ASK,
            "outsideScope",
        );
        assert_decision(&shell("echo hi > dangling"), &scope, ASK, "outsideScope");
        assert_decision(
            &write(&[&tree.path("root/loop/x")]),
            &scope,
            ASK,
            "unresolvedPath",
        );
    }

    #[test]
    fn links_to_credentials_are_credential_paths() {
        let tree = Tree::new();
        tree.link(tree.base.join("outside/.ssh/id_rsa"), "root/innocent.txt");
        let scope = tree.scope();
        assert_decision(
            &read(&[&tree.path("root/innocent.txt")]),
            &scope,
            ASK,
            "credentialPath",
        );
        assert_decision(&shell("cat innocent.txt"), &scope, ASK, "credentialPath");
        assert_decision(&write(&["innocent.txt"]), &scope, ASK, "credentialPath");
    }

    #[test]
    fn credential_stores_under_a_linked_home_are_credential_paths() {
        let tree = Tree::new();
        fs::create_dir_all(tree.base.join("real-home/.config/gh")).expect("gh");
        fs::write(tree.base.join("real-home/.config/gh/hosts.yml"), "token").expect("hosts");
        fs::create_dir_all(tree.base.join("real-home/Library/Keychains")).expect("keychains");
        tree.link(tree.base.join("real-home"), "home");
        tree.link(tree.base.join("real-home/.config"), "root/cfg");
        let scope = tree.scope();
        let real = tree.path("real-home/.config/gh/hosts.yml");
        assert_decision(&read(&[&real]), &scope, ASK, "credentialPath");
        // Only the resolved home names this store: the scope's home is the link.
        let keychain = tree.path("real-home/Library/Keychains/login.keychain-db");
        assert_decision(&read(&[&keychain]), &scope, ASK, "credentialPath");
        assert_decision(&read(&["~/.config/gh"]), &scope, ASK, "credentialPath");
        assert_decision(&shell("cat cfg/gh/*"), &scope, ASK, "credentialPath");
        // A link inside a root that leads out of it is an outside read.
        assert_decision(
            &read(&[&tree.path("root/cfg/other")]),
            &scope,
            ASK,
            "outsideScope",
        );
    }

    #[test]
    fn a_program_reached_through_a_symlink_matches_the_real_binary() {
        let tree = Tree::new();
        fs::create_dir_all(tree.base.join("bin")).expect("bin");
        fs::write(
            tree.base.join("bin/git"),
            "#!/bin/sh
",
        )
        .expect("git");
        fs::write(
            tree.base.join("bin/curl"),
            "#!/bin/sh
",
        )
        .expect("curl");
        // A pre-existing link (not created in this command) resolves to its target.
        tree.link(tree.base.join("bin/git"), "root/g");
        tree.link(tree.base.join("bin/curl"), "root/c");
        let scope = tree.scope();
        assert_decision(
            &shell("./g push origin main"),
            &scope,
            ASK,
            "risky:git-push",
        );
        assert_decision(
            &shell("./c -d a=1 https://x.dev"),
            &scope,
            ASK,
            "risky:curl-upload",
        );
        assert_decision(&shell("./g status"), &scope, ALLOW, "shellInScope");
        // A binary created earlier in the same command is unknowable.
        assert_decision(
            &shell("ln -s bin/curl c2 && ./c2 -d a=1 https://x.dev"),
            &scope,
            ASK,
            "unresolvedPath",
        );
    }

    #[test]
    fn links_into_git_hooks_are_agent_config() {
        let tree = Tree::new();
        fs::create_dir_all(tree.base.join("root/.git/hooks")).expect("hooks");
        tree.link(tree.base.join("root/.git/hooks"), "root/h");
        let scope = tree.scope();
        assert_decision(
            &shell("echo x > h/pre-commit"),
            &scope,
            ASK,
            "risky:agent-config",
        );
        assert_decision(
            &write(&[&tree.path("root/h/pre-push")]),
            &scope,
            ASK,
            "risky:agent-config",
        );
    }

    #[test]
    fn links_into_harness_configuration_are_agent_config() {
        let tree = Tree::new();
        fs::create_dir_all(tree.base.join("root/.claude")).expect("claude");
        tree.link(tree.base.join("root/.claude"), "root/settings-dir");
        let scope = tree.scope();
        let through_link = tree.path("root/settings-dir/settings.json");
        assert_decision(&write(&[&through_link]), &scope, ASK, "risky:agent-config");
        assert_decision(
            &shell("echo '{}' > settings-dir/settings.json"),
            &scope,
            ASK,
            "risky:agent-config",
        );
        assert_decision(&read(&[&through_link]), &scope, ALLOW, "read");
    }

    #[test]
    fn a_root_reached_through_a_link_still_contains_its_files() {
        let tree = Tree::new();
        tree.link(tree.base.join("root"), "alias");
        let scope = AgentScope {
            roots: vec![tree.base.join("alias")],
            cwd: tree.base.join("alias"),
            home_dir: tree.base.join("home"),
        };
        assert_decision(
            &write(&[&tree.path("root/real/a.txt")]),
            &scope,
            ALLOW,
            "inScopeWrite",
        );
        assert_decision(
            &write(&[&tree.path("alias/real/a.txt")]),
            &scope,
            ALLOW,
            "inScopeWrite",
        );
        assert_decision(&shell("rm -rf real"), &scope, ALLOW, "shellInScope");
        assert_decision(&delete(&[&tree.path("root")]), &scope, ASK, "risky:rm-root");
    }

    /// Codex wraps every command in `/bin/zsh -lc '<script>'` (live probe P2). engine-codex
    /// unwraps one such wrapper before building these facts; the scripts below are exactly what
    /// `codex_command_approvals_unwrap_the_live_harness_shell_wrappers` and
    /// `codex_command_approvals_unwrap_exactly_one_posix_quoted_wrapper` produce from the live
    /// strings (this crate cannot depend on engine-codex). The wrapped rows show why.
    #[test]
    fn unwrapped_codex_commands_are_classified_by_their_script() {
        let tree = Tree::new();
        fs::write(tree.base.join("root/MEMORY.md"), "memory").expect("memory");
        let scope = tree.scope();
        let cwd = tree.path("root");
        assert_decision(
            &shell_in("cat MEMORY.md", &cwd),
            &scope,
            ALLOW,
            "shellInScope",
        );
        assert_decision(
            &shell_in("git push origin main", &cwd),
            &scope,
            ASK,
            "risky:git-push",
        );
        // The script is read by the policy's own rules: its exclusion globs name credentials.
        assert_decision(
            &shell_in(
                "rg --files -g '*TODO*' -g '*todo*' -g '!.*' -g '!*.env' -g '!**/*credential*' \
                 -g '!**/*service-account*' -g '!**/*key*'",
                &cwd,
            ),
            &scope,
            ASK,
            "credentialPath",
        );
        for wrapped in [
            "/bin/zsh -lc 'cat MEMORY.md'",
            "/bin/zsh -lc 'git push origin main'",
            // Only one wrapper is removed; a second stays an interpreter.
            "bash -lc 'git push'",
        ] {
            assert_decision(&shell_in(wrapped, &cwd), &scope, ASK, "risky:interpreter");
        }
    }
}
