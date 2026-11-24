//! Thread references (T3 section 9, protocol 63): the prompt-context
//! owner asks this host, at every send, what a `chat` chip's block holds and
//! whether its sender may read it. The rules are
//! `interface_settings::thread_reference`; this gathers the facts (the
//! Threads document, hidden and agent sessions, titles, the journal across a
//! switched thread's chain, whether the sender has team tools).

use std::collections::BTreeSet;
use std::path::PathBuf;

use interface_common::sync::lock;
use interface_contracts::{
    engine::SessionKind,
    ipc::{PromptContextInvalidReason, parse_key},
};
use interface_prompt_context::{PromptContextThreads, ThreadReference, ThreadReferenceRefusal};
use interface_settings::{
    thread_journal::{self, ChainSegment},
    thread_reference::{
        self, AdmittedReference, EXCERPT_JOURNAL_LINES, ReferenceFacts, ReferenceRefusal,
    },
};

use super::ThreadHost;

struct HostFacts<'a> {
    host: &'a ThreadHost,
    hidden: &'a BTreeSet<String>,
}

impl ReferenceFacts for HostFacts<'_> {
    fn excluded(&self, key: &str) -> bool {
        self.hidden.contains(key)
            || self
                .host
                .summary(key)
                .is_some_and(|summary| summary.kind == Some(SessionKind::Agent))
    }
}

const fn refusal(refused: ReferenceRefusal) -> ThreadReferenceRefusal {
    ThreadReferenceRefusal {
        reason: match refused {
            ReferenceRefusal::SenderNotThread => PromptContextInvalidReason::SessionUnavailable,
            ReferenceRefusal::SelfReference => PromptContextInvalidReason::InvalidShape,
            ReferenceRefusal::NotListed => PromptContextInvalidReason::TargetMissing,
            ReferenceRefusal::OtherProject => PromptContextInvalidReason::OutsideAuthorizedRoot,
        },
        message: refused.message(),
    }
}

const UNAVAILABLE: ThreadReferenceRefusal = ThreadReferenceRefusal {
    reason: PromptContextInvalidReason::TargetMissing,
    message: "Threads could not be read just now; nothing was sent. Try again in a moment.",
};

impl ThreadHost {
    /// Records that `key`'s session has the `iface_team` server bound.
    pub(crate) fn team_bound(&self, key: &str, bound: bool) {
        let mut keys = lock(&self.inner.team_bound);
        if bound {
            keys.insert(key.to_owned());
        } else {
            keys.remove(key);
        }
    }

    /// Whether `key`'s session has team tools now.
    #[must_use]
    pub(crate) fn has_team_tools(&self, key: &str) -> bool {
        lock(&self.inner.team_bound).contains(key)
    }

    async fn admitted_reference(
        &self,
        sender: &str,
        target: &str,
    ) -> Result<AdmittedReference, ThreadReferenceRefusal> {
        let hidden = self.inner.ports.hidden.hidden_keys().await;
        let facts = HostFacts {
            host: self,
            hidden: &hidden,
        };
        self.peek(|document, _| thread_reference::admit(document, &facts, sender, target))
            .await
            .map_err(|_| UNAVAILABLE)?
            .map_err(refusal)
    }

    /// The block for `target` as `sender` may read it. Blocking: the journal
    /// is read here.
    fn reference_block(
        &self,
        sender: &str,
        target: &str,
    ) -> Result<ThreadReference, ThreadReferenceRefusal> {
        let handle = tokio::runtime::Handle::try_current().map_err(|_| UNAVAILABLE)?;
        let admitted = handle.block_on(self.admitted_reference(sender, target))?;
        let labels = admitted
            .chain
            .iter()
            .map(|key| {
                parse_key(key)
                    .map(|(engine, _)| super::team::provider_label(engine.as_str(), None))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let events = self.journal_root().map_or_else(Vec::new, |root| {
            let segments = admitted
                .chain
                .iter()
                .zip(&labels)
                .map(|(key, label)| ChainSegment { key, label })
                .collect::<Vec<_>>();
            thread_journal::replay_chain(&root, &segments, EXCERPT_JOURNAL_LINES, None)
        });
        let (prompt, answer) = thread_reference::last_exchange(&events);
        let title = self
            .summary(&admitted.target)
            .map(|summary| summary.title)
            .unwrap_or_default();
        let provider = labels.last().cloned().unwrap_or_default();
        let block = thread_reference::reference_block(
            &thread_reference::reference_title(&title, &admitted.target),
            &admitted.target,
            &provider,
            self.has_team_tools(&admitted.sender),
            &thread_reference::excerpt(prompt.as_deref(), answer.as_deref()),
        );
        Ok(ThreadReference {
            key: admitted.target,
            block,
        })
    }
}

impl PromptContextThreads for ThreadHost {
    fn thread_reference(
        &self,
        sender: &str,
        target: &str,
    ) -> Result<ThreadReference, ThreadReferenceRefusal> {
        self.reference_block(sender, target)
    }

    fn retained_paths(&self) -> Vec<PathBuf> {
        self.stash_retained_paths()
    }
}
