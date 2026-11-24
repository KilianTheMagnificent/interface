//! Provider approvals and question prompts: the pending approval a session
//! row carries, validation of raw question payloads, the keys that answer
//! them, and the answer sent back to the provider.
use interface_contracts::questions::{is_question_tool, parse_agent_questions};
use interface_engine::{ApprovalResponse, ApprovalVerdict};

use super::{
    BTreeMap, BTreeSet, KeyCode, KeyEvent, KeyModifiers, TuiApp, TuiController, for_display,
    read_lock, write_lock,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ApprovalOption {
    pub(super) label: String,
    pub(super) description: String,
    pub(super) preview: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ApprovalQuestion {
    pub(super) question: String,
    pub(super) header: String,
    pub(super) options: Vec<ApprovalOption>,
    pub(super) multi_select: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ApprovalKind {
    Plain,
    Questions {
        questions: Vec<ApprovalQuestion>,
        selections: BTreeMap<usize, Vec<usize>>,
        active_question: usize,
    },
    InvalidQuestions {
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingApproval {
    pub(super) raw_id: String,
    pub(super) tool: String,
    pub(super) summary: String,
    pub(super) kind: ApprovalKind,
}

impl TuiController {
    async fn answer_plain_approval(&self, allow: bool) -> Result<(), String> {
        let key = self
            .active_key()
            .ok_or_else(|| "No active session.".to_owned())?;
        let approval = read_lock(&self.state)
            .sessions
            .get(&key)
            .and_then(|row| row.pending_approval.clone())
            .ok_or_else(|| "No approval is pending.".to_owned())?;
        if allow && !matches!(approval.kind, ApprovalKind::Plain) {
            return Err("This question payload is deny-only or incomplete.".into());
        }
        self.setting_effect(self.runtime.respond_to_approval(
            &key,
            approval.raw_id,
            if allow {
                ApprovalVerdict::Allow
            } else {
                ApprovalVerdict::Deny
            },
            ApprovalResponse::default(),
        ))
        .await?;
        if let Some(session) = write_lock(&self.state).sessions.get_mut(&key) {
            session.pending_approval = None;
        }
        self.changed();
        Ok(())
    }

    async fn submit_question_approval(&self) -> Result<(), String> {
        let key = self
            .active_key()
            .ok_or_else(|| "No active session.".to_owned())?;
        let approval = read_lock(&self.state)
            .sessions
            .get(&key)
            .and_then(|row| row.pending_approval.clone())
            .ok_or_else(|| "No approval is pending.".to_owned())?;
        let ApprovalKind::Questions {
            questions,
            selections,
            ..
        } = &approval.kind
        else {
            return Err("This question payload is deny-only or incomplete.".into());
        };
        let answers = answers_for_questions(questions, selections)
            .ok_or_else(|| "This question payload is deny-only or incomplete.".to_owned())?;
        self.setting_effect(self.runtime.respond_to_approval(
            &key,
            approval.raw_id,
            ApprovalVerdict::Allow,
            ApprovalResponse {
                note: None,
                answers,
            },
        ))
        .await?;
        if let Some(session) = write_lock(&self.state).sessions.get_mut(&key) {
            session.pending_approval = None;
        }
        self.changed();
        Ok(())
    }
}

impl TuiApp {
    pub(super) async fn handle_approval_key(&mut self, key: KeyEvent) -> bool {
        let Some(active) = self.controller.active_key() else {
            return false;
        };
        let command_modifier = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        // Every key passes through here: only a pending approval needs the
        // write lock, which would otherwise hold off drawing and the event pump.
        if read_lock(&self.controller.state)
            .sessions
            .get(&active)
            .is_none_or(|session| session.pending_approval.is_none())
        {
            return false;
        }
        let mut verdict = None;
        let mut submit_questions = false;
        {
            let mut state = write_lock(&self.controller.state);
            let Some(approval) = state
                .sessions
                .get_mut(&active)
                .and_then(|session| session.pending_approval.as_mut())
            else {
                return false;
            };
            if !command_modifier
                && matches!(key.code, KeyCode::Char(character) if character.eq_ignore_ascii_case(&'d'))
            {
                verdict = Some(false);
            } else {
                match &mut approval.kind {
                    ApprovalKind::Plain => {
                        if !command_modifier
                            && matches!(key.code, KeyCode::Char(character) if character.eq_ignore_ascii_case(&'a'))
                        {
                            verdict = Some(true);
                        }
                    }
                    ApprovalKind::InvalidQuestions { .. } => {}
                    ApprovalKind::Questions {
                        questions,
                        selections,
                        active_question,
                    } => {
                        *active_question =
                            (*active_question).min(questions.len().saturating_sub(1));
                        match key.code {
                            KeyCode::Up if *active_question > 0 => *active_question -= 1,
                            KeyCode::Down if *active_question + 1 < questions.len() => {
                                *active_question += 1;
                            }
                            KeyCode::Char(character @ '1'..='4') if !command_modifier => {
                                let option = usize::from(character as u8 - b'1');
                                if let Some(question) = questions.get(*active_question) {
                                    if option < question.options.len() {
                                        if question.multi_select {
                                            let current =
                                                selections.entry(*active_question).or_default();
                                            if let Some(index) =
                                                current.iter().position(|value| *value == option)
                                            {
                                                current.remove(index);
                                            } else {
                                                current.push(option);
                                                current.sort_unstable();
                                            }
                                        } else {
                                            selections.insert(*active_question, vec![option]);
                                        }
                                    }
                                }
                            }
                            KeyCode::Enter if !command_modifier => {
                                submit_questions =
                                    answers_for_questions(questions, selections).is_some();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        if let Some(allow) = verdict {
            self.notice = self.controller.answer_plain_approval(allow).await.err();
        } else if submit_questions {
            self.notice = self.controller.submit_question_approval().await.err();
        } else {
            self.controller.changed();
        }
        true
    }

    pub(super) fn pending_approval(&self) -> Option<PendingApproval> {
        self.controller.active_key().and_then(|key| {
            read_lock(&self.controller.state)
                .sessions
                .get(&key)
                .and_then(|row| row.pending_approval.clone())
        })
    }
}

pub(super) fn build_approval(
    approval_id: &str,
    tool: &str,
    summary: &str,
    input: &serde_json::Value,
) -> PendingApproval {
    let display_tool = for_display(tool);
    let kind = if !is_question_tool(tool) && !is_question_tool(&display_tool) {
        ApprovalKind::Plain
    } else {
        match validate_raw_questions(&display_tool, input) {
            Ok(questions) => ApprovalKind::Questions {
                questions,
                selections: BTreeMap::new(),
                active_question: 0,
            },
            Err(errors) => ApprovalKind::InvalidQuestions {
                reason: if errors.is_empty() {
                    "The raw question payload is malformed.".into()
                } else {
                    errors.join(" ")
                },
            },
        }
    };
    PendingApproval {
        raw_id: approval_id.to_owned(),
        tool: display_tool,
        summary: for_display(summary),
        kind,
    }
}

#[allow(clippy::too_many_lines)]
fn validate_raw_questions(
    tool: &str,
    input: &serde_json::Value,
) -> Result<Vec<ApprovalQuestion>, Vec<String>> {
    let Some(raw_questions) = input
        .as_object()
        .and_then(|record| record.get("questions"))
        .and_then(serde_json::Value::as_array)
    else {
        return Err(vec!["Questions must be an array.".into()]);
    };
    let mut errors = Vec::new();
    if !(1..=4).contains(&raw_questions.len()) {
        errors.push("Question approvals require between 1 and 4 questions.".into());
    }
    let mut questions = Vec::new();
    let mut raw_question_text = BTreeSet::new();
    let mut display_question_text = BTreeSet::new();
    for (question_index, raw_question) in raw_questions.iter().enumerate() {
        let number = question_index + 1;
        let Some(record) = raw_question.as_object() else {
            errors.push(format!("Question {number} must be an object."));
            continue;
        };
        let question = record
            .get("question")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty());
        let header = record
            .get("header")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty());
        let multi_select = record
            .get("multiSelect")
            .and_then(serde_json::Value::as_bool);
        if question.is_none() {
            errors.push(format!("Question {number} has an invalid question."));
        }
        if header.is_none() {
            errors.push(format!("Question {number} has an invalid header."));
        }
        if multi_select.is_none() {
            errors.push(format!(
                "Question {number} has an invalid multiSelect flag."
            ));
        }
        let Some(raw_options) = record.get("options").and_then(serde_json::Value::as_array) else {
            errors.push(format!("Question {number} options must be an array."));
            continue;
        };
        if !(2..=4).contains(&raw_options.len()) {
            errors.push(format!(
                "Question {number} requires between 2 and 4 options."
            ));
        }
        let mut options = Vec::new();
        let mut raw_labels = BTreeSet::new();
        let mut display_labels = BTreeSet::new();
        for (option_index, raw_option) in raw_options.iter().enumerate() {
            let option_number = option_index + 1;
            let Some(option) = raw_option.as_object() else {
                errors.push(format!(
                    "Question {number}, option {option_number} must be an object."
                ));
                continue;
            };
            let label = option
                .get("label")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty());
            let description = option
                .get("description")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty());
            let preview_present = option.contains_key("preview");
            let preview = option.get("preview").and_then(serde_json::Value::as_str);
            if label.is_none() {
                errors.push(format!(
                    "Question {number}, option {option_number} has an invalid label."
                ));
            }
            if description.is_none() {
                errors.push(format!(
                    "Question {number}, option {option_number} has an invalid description."
                ));
            }
            if preview_present && preview.is_none() {
                errors.push(format!(
                    "Question {number}, option {option_number} has an invalid preview."
                ));
            }
            if let Some(label) = label {
                if !raw_labels.insert(label.to_owned()) {
                    errors.push(format!("Question {number} has a duplicate option label."));
                }
                let display = for_display(label);
                if display.is_empty() {
                    errors.push(format!(
                        "Question {number} has an option label that is empty after display sanitization."
                    ));
                } else if !display_labels.insert(display) {
                    errors.push(format!(
                        "Question {number} has option labels that are indistinguishable after display sanitization."
                    ));
                }
            }
            if let (Some(label), Some(description)) = (label, description) {
                if !preview_present || preview.is_some() {
                    options.push(ApprovalOption {
                        label: label.to_owned(),
                        description: description.to_owned(),
                        preview: preview.map(str::to_owned),
                    });
                }
            }
        }
        if let Some(question) = question {
            if !raw_question_text.insert(question.to_owned()) {
                errors.push("Question text must be unique.".into());
            }
            let display = for_display(question);
            if display.is_empty() {
                errors.push("Question text cannot be empty after display sanitization.".into());
            } else if !display_question_text.insert(display) {
                errors.push("Question text must remain unique after display sanitization.".into());
            }
        }
        if let (Some(question), Some(header), Some(multi_select)) = (question, header, multi_select)
        {
            if options.len() == raw_options.len() {
                questions.push(ApprovalQuestion {
                    question: question.to_owned(),
                    header: header.to_owned(),
                    options,
                    multi_select,
                });
            }
        }
    }
    let display_parsed = parse_agent_questions(tool, input);
    if display_parsed
        .as_ref()
        .is_none_or(|parsed| parsed.len() != raw_questions.len())
    {
        errors.push("The display parser did not preserve every raw question.".into());
    }
    if questions.len() != raw_questions.len() {
        errors.push("One or more raw questions could not be represented safely.".into());
    }
    if errors.is_empty() {
        Ok(questions)
    } else {
        Err(errors)
    }
}

pub(super) fn answers_for_questions(
    questions: &[ApprovalQuestion],
    selections: &BTreeMap<usize, Vec<usize>>,
) -> Option<Vec<(String, String)>> {
    if questions.is_empty() {
        return None;
    }
    let mut answers = Vec::with_capacity(questions.len());
    for (question_index, question) in questions.iter().enumerate() {
        let selected = selections.get(&question_index)?;
        if selected.is_empty() {
            return None;
        }
        let unique = selected.iter().copied().collect::<BTreeSet<_>>();
        if (!question.multi_select && unique.len() != 1)
            || unique.iter().any(|index| *index >= question.options.len())
        {
            return None;
        }
        answers.push((
            question.question.clone(),
            unique
                .into_iter()
                .map(|index| question.options[index].label.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ));
    }
    Some(answers)
}
