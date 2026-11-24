use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentQuestionOption {
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<AgentQuestionOption>,
    pub multi_select: bool,
}

#[must_use]
pub fn is_question_tool(tool: &str) -> bool {
    let normalized = tool.to_ascii_lowercase();
    ["askuserquestion", "ask_user_question", "askquestion"]
        .iter()
        .any(|candidate| normalized.contains(candidate))
}

fn nonempty_text(value: Option<&Value>) -> Option<&str> {
    value?.as_str().filter(|text| !text.is_empty())
}

fn parse_question(value: &Value) -> Option<AgentQuestion> {
    let record = value.as_object()?;
    let question = nonempty_text(record.get("question"))?.to_owned();
    let options = record
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let option = option.as_object()?;
            Some(AgentQuestionOption {
                label: nonempty_text(option.get("label"))?.to_owned(),
                description: nonempty_text(option.get("description"))
                    .unwrap_or("")
                    .to_owned(),
                preview: nonempty_text(option.get("preview")).map(str::to_owned),
            })
        })
        .collect::<Vec<_>>();
    if options.len() < 2 {
        return None;
    }
    let header = nonempty_text(record.get("header"))
        .map_or_else(|| question.chars().take(12).collect(), str::to_owned);
    Some(AgentQuestion {
        question,
        header,
        options,
        multi_select: record.get("multiSelect").and_then(Value::as_bool) == Some(true),
    })
}

#[must_use]
pub fn parse_agent_questions(tool: &str, input: &Value) -> Option<Vec<AgentQuestion>> {
    let raw = input.as_object().and_then(|record| record.get("questions"));
    let Some(entries) = raw.and_then(Value::as_array) else {
        return is_question_tool(tool).then(Vec::new);
    };
    if entries.is_empty() {
        return is_question_tool(tool).then(Vec::new);
    }
    let questions = entries
        .iter()
        .filter_map(parse_question)
        .collect::<Vec<_>>();
    if questions.is_empty() && !is_question_tool(tool) {
        None
    } else {
        Some(questions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cross_provider_question_shape() {
        let parsed = parse_agent_questions("AskUserQuestion", &json!({"questions":[{
            "question":"Choose one", "options":[{"label":"A"},{"label":"B","description":"second"}],
            "multiSelect":true
        }]})).unwrap();
        assert_eq!(parsed[0].header, "Choose one");
        assert_eq!(parsed[0].options[0].description, "");
        assert!(parsed[0].multi_select);
        assert_eq!(
            serde_json::to_value(&parsed[0]).unwrap()["multiSelect"],
            true
        );
    }

    #[test]
    fn non_question_tools_do_not_gain_empty_question_state() {
        assert_eq!(parse_agent_questions("Read", &json!({})), None);
        assert_eq!(
            parse_agent_questions("askquestion", &json!({})),
            Some(vec![])
        );
    }
}
