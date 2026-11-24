use super::*;
use interface_contracts::orchestration::{MessageOrigin, create_empty_orchestration_snapshot};

#[derive(Serialize)]
struct TestPage {
    items: Vec<String>,
    removed: usize,
    cursor: Option<String>,
    has_more: bool,
}

fn page(items: &[String], removed: usize, contents: PageContents) -> TestPage {
    TestPage {
        items: contents.entries(items),
        removed,
        cursor: items
            .first()
            .map(|item| item.chars().take(30 + removed % 7).collect()),
        has_more: removed % 2 == 0,
    }
}

#[test]
#[ignore = "manual near-capacity serialization benchmark"]
fn benchmark_page_budget_selection() {
    let selected = vec!["\"界\n".repeat(2_000); 100];
    let started = std::time::Instant::now();
    let actual = shrink_page_to_budget(&selected, 1, TrimEnd::Oldest, page, "fixture").unwrap();
    let optimized = started.elapsed();
    let started = std::time::Instant::now();
    let mut items = selected;
    let mut removed = 0;
    let expected = loop {
        let candidate = page(&items, removed, PageContents::Include);
        if bytes(&candidate).unwrap() <= ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES {
            break candidate;
        }
        items.remove(0);
        removed += 1;
    };
    let reference = started.elapsed();
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    eprintln!(
        "near-capacity page: reference={reference:?}, optimized={optimized:?}, retained={}, encoded={}",
        actual.items.len(),
        bytes(&actual).unwrap()
    );
}

#[test]
fn exact_page_accounting_matches_full_serialization() {
    for seed in 0..32 {
        let selected = (0..12)
            .map(|index| {
                let text = ["plain", "\\\"\n界", "\tend"][index % 3];
                text.repeat(1_000 + (seed * 997 + index * 701) % 14_000)
            })
            .collect::<Vec<_>>();
        for end in [TrimEnd::Oldest, TrimEnd::Newest] {
            for floor in [0, 1] {
                let mut expected_items = selected.clone();
                let mut removed = 0;
                let expected = loop {
                    let candidate = page(&expected_items, removed, PageContents::Include);
                    if bytes(&candidate).unwrap() <= ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES
                        || expected_items.len() <= floor
                    {
                        break assert_orchestration_control_public_budget(&candidate, "fixture")
                            .map(|()| serde_json::to_vec(&candidate).unwrap());
                    }
                    if end == TrimEnd::Oldest {
                        expected_items.remove(0);
                    } else {
                        expected_items.pop();
                    }
                    removed += 1;
                };
                let actual = shrink_page_to_budget(&selected, floor, end, page, "fixture")
                    .map(|value| serde_json::to_vec(&value).unwrap());
                assert_eq!(actual.is_ok(), expected.is_ok());
                if let (Ok(actual), Ok(expected)) = (actual, expected) {
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

#[test]
fn snapshot_accounting_matches_reference_including_one_byte_boundary() {
    let empty = create_empty_orchestration_snapshot(
        "host".into(),
        "epoch".into(),
        "2026-01-01T00:00:00.000Z".into(),
    )
    .unwrap();
    let base = project_orchestration_snapshot_for_control(&empty).unwrap();
    for seed in 0..24 {
        let mut actual = base.clone();
        actual.projection.omitted.messages = [0, 9, 99, 999][seed % 4];
        actual.projection.truncated = seed % 2 == 0;
        actual.messages = (0..24)
            .map(|index| OrchestrationControlMessageSummaryV1 {
                version: 1,
                message_id: "\n界".repeat(3_000 + (seed * 97 + index * 103) % 5_000),
                thread_id: "thread".into(),
                parent_message_id: None,
                origin: MessageOrigin::Human,
                sender_agent_id: None,
                recipient_agent_id: "agent".into(),
                expect_reply: true,
                delivery_state: MessageDeliveryState::Requested,
                created_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
            })
            .collect();
        let mut expected = actual.clone();
        let a = trim_snapshot(&mut actual);
        let b = trim_snapshot_reference(&mut expected);
        assert_eq!(a.is_ok(), b.is_ok());
        assert_eq!(
            serde_json::to_vec(&actual).unwrap(),
            serde_json::to_vec(&expected).unwrap()
        );
    }
    let mut actual = base;
    actual.host_id.push_str(
        &"x".repeat(ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES + 1 - bytes(&actual).unwrap()),
    );
    let mut expected = actual.clone();
    assert!(trim_snapshot(&mut actual).is_err());
    assert!(trim_snapshot_reference(&mut expected).is_err());
}

enum Removal {
    Messages,
    RoleClaims,
    WorktreeLeases,
    Agents,
    Policies,
}

fn trim_snapshot_reference(projected: &mut OrchestrationControlSnapshotV1) -> Result<()> {
    let order = [
        Removal::Messages,
        Removal::RoleClaims,
        Removal::WorktreeLeases,
        Removal::Agents,
        Removal::Policies,
    ];
    while bytes(projected)? > ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES {
        let mut removed = false;
        for key in &order {
            let did = match key {
                Removal::Messages if !projected.messages.is_empty() => {
                    projected.messages.remove(0);
                    projected.projection.omitted.messages += 1;
                    true
                }
                Removal::RoleClaims if !projected.role_claims.is_empty() => {
                    projected.role_claims.remove(0);
                    projected.projection.omitted.role_claims += 1;
                    true
                }
                Removal::WorktreeLeases if !projected.worktree_leases.is_empty() => {
                    projected.worktree_leases.remove(0);
                    projected.projection.omitted.worktree_leases += 1;
                    true
                }
                Removal::Agents if !projected.agents.is_empty() => {
                    projected.agents.remove(0);
                    projected.projection.omitted.agents += 1;
                    true
                }
                Removal::Policies if !projected.orchestrator_policies.is_empty() => {
                    projected.orchestrator_policies.remove(0);
                    projected.projection.omitted.orchestrator_policies += 1;
                    true
                }
                _ => false,
            };
            if did {
                removed = true;
                break;
            }
        }
        if !removed {
            return Err(OrchestrationError::new(
                OrchestrationErrorCode::PublicBudget,
                "Minimal orchestration control snapshot exceeds its wire budget.",
            ));
        }
        projected.projection.truncated = true;
    }
    Ok(())
}
