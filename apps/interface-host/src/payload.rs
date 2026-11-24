//! The request-payload rules the owner dispatchers share. Each owner passes
//! its own refusal, so every wire code and message stays the owner's own.

use serde::de::DeserializeOwned;
use serde_json::Value;

/// Whether a payload carries no arguments at all: `null` or `{}`.
#[must_use]
pub fn is_empty(payload: &Value) -> bool {
    payload.is_null() || payload.as_object().is_some_and(serde_json::Map::is_empty)
}

/// Accepts only an empty payload (see [`is_empty`]).
///
/// # Errors
///
/// Returns `refuse()` for any other payload.
pub(crate) fn require_empty<E>(payload: &Value, refuse: impl FnOnce() -> E) -> Result<(), E> {
    if is_empty(payload) {
        Ok(())
    } else {
        Err(refuse())
    }
}

/// Decodes a payload that must be a JSON object. Any other JSON value is
/// refused before serde sees it (with `None`), since a struct would also
/// accept an array; a serde failure is refused with its error.
///
/// # Errors
///
/// Returns `refuse(None)` for a non-object and `refuse(Some(error))` for an
/// object that does not decode.
pub(crate) fn decode_object<T, E>(
    payload: Value,
    refuse: impl FnOnce(Option<serde_json::Error>) -> E,
) -> Result<T, E>
where
    T: DeserializeOwned,
{
    if !payload.is_object() {
        return Err(refuse(None));
    }
    serde_json::from_value(payload).map_err(|error| refuse(Some(error)))
}
