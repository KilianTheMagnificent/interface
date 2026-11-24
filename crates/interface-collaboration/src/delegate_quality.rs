//! Worker claims are untrusted; actual file edits determine proposal eligibility.
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Report {
    version: u8,
    outcome: String,
    summary: String,
    missing_files: Vec<String>,
}

// OpenCode may combine commentary and a fenced final JSON report in one
// assistant step. Accept only a complete terminal object, never fields scraped
// from prose, an earlier report, or a report followed by additional commentary.
fn terminal_report(summary: &str) -> Option<Report> {
    let terminal = summary.trim();
    if terminal.len() > 16 * 1024 {
        return None;
    }
    let body = if let Some(rest) = terminal.strip_suffix("```") {
        let rest = rest.trim_end();
        let opening = rest
            .rfind("\n```")
            .map(|i| i + 1)
            .or_else(|| rest.starts_with("```").then_some(0))?;
        let (language, json) = rest[opening + 3..].split_once('\n')?;
        if !matches!(language.trim(), "json" | "") {
            return None;
        }
        json.trim()
    } else {
        terminal
    };
    serde_json::from_str::<Report>(body).ok().or_else(|| {
        body.rmatch_indices('\n')
            .filter_map(|(index, _)| {
                let suffix = body[index + 1..].trim_start();
                suffix.starts_with('{').then_some(suffix)
            })
            .take(32)
            .find_map(|suffix| serde_json::from_str::<Report>(suffix).ok())
    })
}

/// Returns a versioned outcome and a recovery hint, without granting access to
/// any worker-requested file. The parent must review and supply fresh context.
#[must_use]
pub fn worker_outcome(summary: &str, has_changes: bool) -> Value {
    let parsed = terminal_report(summary);
    let report = parsed.filter(|r| {
        r.version == 1
            && !r.summary.trim().is_empty()
            && r.summary.len() <= 4096
            && r.missing_files.len() <= 48
            && r.missing_files
                .iter()
                .all(|p| !p.is_empty() && p.len() <= 1024 && !p.chars().any(char::is_control))
            && matches!(r.outcome.as_str(), "changes" | "needsContext" | "blocked")
            && ((r.outcome == "needsContext") != r.missing_files.is_empty())
    });
    let Some(report) = report else {
        return json!({"version":1,"outcome":"invalidReport","eligibleForApproval":false,"eligibleForValidation":has_changes,
            "summary":"Worker did not return a valid outcome report. Inspect actual edits for completeness; reject partial work. Complete edits can be validated, but approval requires explicit acceptUnreportedChanges and passing host checks.",
            "missingFiles":[],"recovery":{"action":"inspectEditsThenValidateOrReject","approvalRequires":"acceptUnreportedChangesAndPassingHostValidation","newRequestId":true}});
    };
    let outcome = if report.outcome == "changes" {
        if has_changes { "proposal" } else { "noChanges" }
    } else {
        &report.outcome
    };
    json!({"version":1,"outcome":outcome,"eligibleForApproval":outcome == "proposal","eligibleForValidation":outcome == "proposal",
        "summary":report.summary,"missingFiles":report.missing_files,
        "recovery": if outcome == "proposal" { json!({"action":"validateThenReview"}) }
          else { json!({"action":"reviewThenSubmitFreshTask","newRequestId":true,
            "contextAuthority":"parentMustApproveExactSafeFiles","partialEdits":"notApplied"}) }})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_opencode_plain_and_fenced_terminal_reports_remain_structured() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/delegation-worker-reports-v1.json"
        ))
        .unwrap();
        for row in fixture["reports"].as_array().unwrap() {
            let summary = row["summary"].as_str().unwrap();
            assert_eq!(
                worker_outcome(summary, row["hasChanges"].as_bool().unwrap())["outcome"],
                row["expectedOutcome"]
            );
            assert_eq!(
                worker_outcome(&format!("{summary}\nIncomplete after all"), true)["eligibleForApproval"],
                false
            );
            assert_eq!(
                worker_outcome(summary, false)["outcome"],
                if row["expectedOutcome"] == "proposal" {
                    "noChanges"
                } else {
                    "invalidReport"
                }
            );
        }
    }

    #[test]
    fn prose_empty_edits_and_blockers_never_become_usable_proposals() {
        assert_eq!(worker_outcome("Done", true)["outcome"], "invalidReport");
        let report = json!({"version":1,"outcome":"changes","summary":"Added implementation","missingFiles":[]}).to_string();
        assert_eq!(worker_outcome(&report, false)["outcome"], "noChanges");
        assert_eq!(worker_outcome(&report, true)["eligibleForApproval"], true);
        assert_eq!(
            worker_outcome(&format!("Reviewed the edit.\n\n{report}"), true)["eligibleForApproval"],
            true
        );
        assert_eq!(
            worker_outcome(&format!("{report}\nActually blocked"), true)["outcome"],
            "invalidReport"
        );
        assert_eq!(
            worker_outcome(&format!("Reviewed the edit.\n{report}"), false)["outcome"],
            "noChanges"
        );
        for prefix in ["", "Reviewed the implementation.\n\n"] {
            for fence in ["```json", "```"] {
                assert_eq!(
                    worker_outcome(&format!("{prefix}{fence}\n{report}\n```"), true)["eligibleForApproval"],
                    true
                );
            }
        }
        assert_eq!(
            worker_outcome(&format!("{report}\n```"), true)["outcome"],
            "invalidReport"
        );
        assert_eq!(
            worker_outcome(&format!("```python\n{report}\n```"), true)["outcome"],
            "invalidReport"
        );
        let missing = json!({"version":1,"outcome":"needsContext","summary":"Need parser behavior","missingFiles":["src/normalize.js"]}).to_string();
        assert_eq!(worker_outcome(&missing, true)["eligibleForApproval"], false);
        assert_eq!(
            worker_outcome(&missing, true)["missingFiles"][0],
            "src/normalize.js"
        );
    }
}
