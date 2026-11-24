//! Local runner preferences. The desktop keeps its existing Rust owner alive;
//! this never starts a second process or changes routine authorization.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum BackgroundCommand {
    Read,
    Set { enabled: bool },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackgroundState {
    pub enabled: bool,
    pub available: bool,
}
