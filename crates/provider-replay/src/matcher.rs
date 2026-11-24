//! How an arriving frame is matched to a recorded `out` frame, which values
//! it binds, and how bound values are put into the recorded `in` frames.
//!
//! A frame matches by its signature (JSON-RPC method, or Claude's `type` and
//! control `subtype`) and its shape (the same object keys and array lengths
//! all the way down, the same JSON types at the leaves). Leaf values are not
//! compared: the host makes some of them fresh every run (JSON-RPC request
//! ids, Claude `request_id`s, session ids, temp paths), so a recorded string
//! that differs is bound to the one that arrived, and a recorded request id
//! to the id the host used.

use std::{collections::BTreeMap, fmt::Write as _};

use serde_json::{Map, Value};

/// A bound string at least this long is also replaced inside longer strings
/// (a temp path inside a tool's output, a session id inside a file name).
/// Shorter ones are replaced only where a whole string equals them.
pub const SUBSTRING_BINDING_MIN: usize = 16;

/// Values bound so far, recorded → arrived.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bindings {
    pub strings: BTreeMap<String, String>,
    /// JSON-RPC ids of requests the host sent.
    pub ids: BTreeMap<String, Value>,
}

impl Bindings {
    /// Binds `recorded` to `arrived`. The first binding of a value stands;
    /// an empty recorded string is never bound (it would replace every
    /// empty string the provider sends).
    pub fn bind_string(&mut self, recorded: &str, arrived: &str) {
        if !recorded.is_empty() && recorded != arrived {
            self.strings
                .entry(recorded.to_owned())
                .or_insert_with(|| arrived.to_owned());
        }
    }
}

/// What kind of frame this is, for matching and for messages:
/// `request initialize`, `response`, `notification turn/started`,
/// `control_request interrupt`, `user`, …
#[must_use]
pub fn signature(frame: &Value) -> String {
    let Value::Object(object) = frame else {
        return match frame {
            Value::String(_) => "text line".to_owned(),
            _ => "non-object frame".to_owned(),
        };
    };
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        let kind = if object.contains_key("id") {
            "request"
        } else {
            "notification"
        };
        return format!("{kind} {method}");
    }
    if object.contains_key("id") && (object.contains_key("result") || object.contains_key("error"))
    {
        let outcome = if object.contains_key("error") {
            "error"
        } else {
            "result"
        };
        return format!("response ({outcome})");
    }
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return "untyped object".to_owned();
    };
    let nested = match kind {
        "control_request" => object.get("request"),
        "control_response" => object.get("response"),
        _ => None,
    };
    let subtype = nested
        .and_then(|nested| nested.get("subtype"))
        .or_else(|| object.get("subtype"))
        .and_then(Value::as_str);
    subtype.map_or_else(|| kind.to_owned(), |subtype| format!("{kind} {subtype}"))
}

/// Where `arrived` departs from `recorded`'s signature or shape, or `None`
/// when it matches.
#[must_use]
pub fn difference(recorded: &Value, arrived: &Value) -> Option<String> {
    let (expected, received) = (signature(recorded), signature(arrived));
    if expected != received {
        return Some(format!("expected a {expected}, received a {received}"));
    }
    shape_difference(recorded, arrived, "$")
}

fn shape_difference(recorded: &Value, arrived: &Value, path: &str) -> Option<String> {
    match (recorded, arrived) {
        (Value::Object(recorded), Value::Object(arrived)) => {
            let missing: Vec<&String> = recorded
                .keys()
                .filter(|key| !arrived.contains_key(*key))
                .collect();
            let extra: Vec<&String> = arrived
                .keys()
                .filter(|key| !recorded.contains_key(*key))
                .collect();
            if !missing.is_empty() || !extra.is_empty() {
                let mut message = format!("at {path}:");
                if !missing.is_empty() {
                    let _ = write!(message, " missing {missing:?}");
                }
                if !extra.is_empty() {
                    let _ = write!(message, " unexpected {extra:?}");
                }
                return Some(message);
            }
            recorded.iter().find_map(|(key, value)| {
                shape_difference(value, &arrived[key], &format!("{path}.{key}"))
            })
        }
        (Value::Array(recorded), Value::Array(arrived)) => {
            if recorded.len() != arrived.len() {
                return Some(format!(
                    "at {path}: expected {} items, received {}",
                    recorded.len(),
                    arrived.len()
                ));
            }
            recorded
                .iter()
                .zip(arrived)
                .enumerate()
                .find_map(|(index, (recorded, arrived))| {
                    shape_difference(recorded, arrived, &format!("{path}[{index}]"))
                })
        }
        _ if kind(recorded) == kind(arrived) => None,
        _ => Some(format!(
            "at {path}: expected {}, received {}",
            kind(recorded),
            kind(arrived)
        )),
    }
}

const fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Binds every leaf string of a matched frame that differs from the
/// recording, and a host request's JSON-RPC id. Call only after
/// [`difference`] answered `None`.
pub fn bind(recorded: &Value, arrived: &Value, bindings: &mut Bindings) {
    if let (Value::Object(recorded_object), Value::Object(arrived_object)) = (recorded, arrived)
        && recorded_object.contains_key("method")
        && let (Some(recorded_id), Some(arrived_id)) =
            (recorded_object.get("id"), arrived_object.get("id"))
        && recorded_id != arrived_id
    {
        bindings
            .ids
            .entry(id_key(recorded_id))
            .or_insert_with(|| arrived_id.clone());
    }
    bind_leaves(recorded, arrived, bindings);
}

fn bind_leaves(recorded: &Value, arrived: &Value, bindings: &mut Bindings) {
    match (recorded, arrived) {
        (Value::Object(recorded), Value::Object(arrived)) => {
            for (key, value) in recorded {
                if let Some(other) = arrived.get(key) {
                    bind_leaves(value, other, bindings);
                }
            }
        }
        (Value::Array(recorded), Value::Array(arrived)) => {
            for (value, other) in recorded.iter().zip(arrived) {
                bind_leaves(value, other, bindings);
            }
        }
        (Value::String(recorded), Value::String(arrived)) => {
            bindings.bind_string(recorded, arrived);
        }
        _ => {}
    }
}

fn id_key(id: &Value) -> String {
    id.to_string()
}

/// A recorded `in` frame with the bound values put in: the id of a
/// response to a host request, and every bound string.
#[must_use]
pub fn substitute(frame: &Value, bindings: &Bindings) -> Value {
    let mut frame = substitute_strings(frame, bindings);
    if let Value::Object(object) = &mut frame
        && !object.contains_key("method")
        && (object.contains_key("result") || object.contains_key("error"))
        && let Some(id) = object.get("id")
        && let Some(bound) = bindings.ids.get(&id_key(id))
    {
        object.insert("id".to_owned(), bound.clone());
    }
    frame
}

fn substitute_strings(value: &Value, bindings: &Bindings) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), substitute_strings(value, bindings)))
                .collect::<Map<String, Value>>(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|value| substitute_strings(value, bindings))
                .collect(),
        ),
        Value::String(text) => Value::String(substitute_text(text, bindings)),
        other => other.clone(),
    }
}

/// `text` with every bound string replaced: whole-string bindings first,
/// then long bindings inside it, longest first.
#[must_use]
pub fn substitute_text(text: &str, bindings: &Bindings) -> String {
    if let Some(bound) = bindings.strings.get(text) {
        return bound.clone();
    }
    let mut long: Vec<(&String, &String)> = bindings
        .strings
        .iter()
        .filter(|(recorded, _)| recorded.len() >= SUBSTRING_BINDING_MIN)
        .collect();
    long.sort_by_key(|(recorded, _)| std::cmp::Reverse(recorded.len()));
    let mut text = text.to_owned();
    for (recorded, arrived) in long {
        if text.contains(recorded.as_str()) {
            text = text.replace(recorded.as_str(), arrived);
        }
    }
    text
}

/// One frame for a message, cut to `limit` characters.
#[must_use]
pub fn excerpt(frame: &Value, limit: usize) -> String {
    let text = match frame {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if text.chars().count() <= limit {
        return text;
    }
    let cut: String = text.chars().take(limit).collect();
    format!("{cut}… ({} characters)", text.chars().count())
}
