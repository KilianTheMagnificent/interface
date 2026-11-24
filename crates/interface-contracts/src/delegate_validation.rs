//! Version 1 of the pane MCP validation protocol, independent of desktop IPC.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationPlan {
    pub version: u8,
    /// Exact safe source inputs; changed paths are included automatically.
    pub files: Vec<String>,
    /// Parent-owned acceptance inputs the proposal must not change.
    pub protected_files: Vec<String>,
    /// Trusted parent-selected argv, executed directly in a disposable copy.
    pub checks: Vec<Vec<String>>,
}

impl ValidationPlan {
    /// # Errors
    /// Rejects unsupported versions, empty acceptance plans and oversized argv.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.files.is_empty()
            || self.files.len() > 512
            || self.protected_files.is_empty()
            || self.protected_files.len() > 512
            || self.protected_files.iter().any(|p| !self.files.contains(p))
            || self.checks.is_empty()
            || self.checks.len() > 8
            || self.checks.iter().any(|argv| {
                argv.is_empty()
                    || argv.len() > 64
                    || argv[0].trim().is_empty()
                    || argv.iter().any(|s| s.len() > 4096 || s.contains('\0'))
            })
            || self.checks.iter().flatten().map(String::len).sum::<usize>() > 32 * 1024
        {
            return Err("Validation v1 requires 1-512 exact input files, protected acceptance files within those inputs, and 1-8 bounded parent-selected argv commands".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationRequest {
    pub task_id: String,
    pub result_sha256: String,
    pub plan: ValidationPlan,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation_mcp_v1_round_trips_the_cross_language_fixture_and_rejects_bad_plans() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/delegation-validation-v1.json"
        ))
        .unwrap();
        let request: ValidationRequest =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        request.plan.validate().unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), fixture["request"]);
        let mut bad = request.plan.clone();
        bad.version = 2;
        assert!(bad.validate().is_err());
        bad = request.plan.clone();
        bad.protected_files.clear();
        assert!(bad.validate().is_err());
        bad = request.plan.clone();
        bad.protected_files.push("missing.test.js".into());
        assert!(bad.validate().is_err());
        bad = request.plan;
        bad.checks = vec![vec![]];
        assert!(bad.validate().is_err());
    }
}
