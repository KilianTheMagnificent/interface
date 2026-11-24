use interface_collaboration::state::{
    COLLABORATION_ERROR_CODES, CollaborationEffectKind, CollaborationEngineId,
    CollaborationErrorCode, CollaborationIntentInput, CollaborationMode, CollaborationOrigin,
    CollaborationPolicyOverrides, CollaborationState, CollaborationTransitionPatch,
    CollaborationTransitionRequest, ContractErrorReason, CreateCollaborationRecordOptions,
    create_collaboration_record, dedupe_key_for_intent, effect_inputs_required,
    fingerprint_origin_lineage, legal_collaboration_transitions, normalize_collaboration_intent,
    payload_hash_for_intent, transition_collaboration_record,
};
use serde_json::json;

const AT: &str = "2026-08-27T12:00:00.000Z";

fn human_input() -> CollaborationIntentInput {
    CollaborationIntentInput {
        version: None,
        mode: CollaborationMode::Consult,
        source_session_key: "claude#source-1".to_owned(),
        target_engine: CollaborationEngineId::Codex,
        objective: "\r\n Hello\r\nworld \r\n".to_owned(),
        client_request_id: "request-1".to_owned(),
        origin: CollaborationOrigin::Human,
    }
}

fn requested_record() -> interface_collaboration::state::CollaborationRecordV1 {
    let intent = normalize_collaboration_intent(&human_input(), None).expect("normalize");
    create_collaboration_record(CreateCollaborationRecordOptions {
        collaboration_id: "collaboration-1".to_owned(),
        intent,
        created_at: AT.to_owned(),
        depth: 0,
        limits: CollaborationPolicyOverrides::default(),
    })
    .expect("record")
}

#[test]
fn intent_normalization_and_hashes_match_legacy_golden_values() {
    let intent = normalize_collaboration_intent(&human_input(), None).expect("normalize");
    assert_eq!(intent.objective.as_deref(), Some("Hello\nworld"));
    assert_eq!(
        intent.objective_digest,
        "46e0ea795802f17d0b340983ca7d7068c94d7d9172ee4daea37a1ab1168649ec"
    );
    assert_eq!(
        payload_hash_for_intent(&intent).expect("payload"),
        "d529d0e3eae50f1f2186683fa2005a7a5b5d563f0e3bd7bbe5414a3a1d7de521"
    );
    assert_eq!(
        dedupe_key_for_intent(&intent).expect("dedupe"),
        "cad79e2f1d9f84362edfbd31229b113e740cf071e23b21bd12ed1158b4c91dd7"
    );
    assert_eq!(
        serde_json::to_value(&intent).expect("json"),
        json!({
            "version": 1,
            "mode": "consult",
            "sourceSessionKey": "claude#source-1",
            "targetEngine": "codex",
            "objectiveDigest": "46e0ea795802f17d0b340983ca7d7068c94d7d9172ee4daea37a1ab1168649ec",
            "objectiveBytes": 11,
            "objectiveCharacters": 11,
            "objective": "Hello\nworld",
            "clientRequestId": "request-1",
            "origin": "human"
        })
    );
}

#[test]
fn objective_is_normalized_to_nfc_but_opaque_identifiers_must_arrive_normalized() {
    let mut input = human_input();
    input.objective = "Cafe\u{301}".to_owned();
    let intent = normalize_collaboration_intent(&input, None).expect("NFC objective");
    assert_eq!(intent.objective.as_deref(), Some("Caf\u{e9}"));
    assert_eq!(intent.objective_characters, 4);
    assert_eq!(intent.objective_bytes, 5);

    input.source_session_key = "claude#cafe\u{301}".to_owned();
    assert!(normalize_collaboration_intent(&input, None).is_err());

    let mut input = human_input();
    input.objective = "\u{feff}Bridge objective\u{feff}".to_owned();
    let intent = normalize_collaboration_intent(&input, None).expect("ECMAScript trim");
    assert_eq!(intent.objective.as_deref(), Some("Bridge objective"));
    input.client_request_id = "\u{feff}request-1".to_owned();
    assert!(normalize_collaboration_intent(&input, None).is_err());
}

#[test]
fn model_lineage_is_required_and_capability_is_not_in_the_wire_record() {
    let mut input = human_input();
    input.origin = CollaborationOrigin::Model;
    let error = normalize_collaboration_intent(&input, None).expect_err("lineage required");
    assert_eq!(error.reason, ContractErrorReason::InvalidContract);

    let intent =
        normalize_collaboration_intent(&input, Some("stable-lineage-01")).expect("model intent");
    assert_eq!(
        intent.origin_lineage_fingerprint.as_deref(),
        Some("b351e59a645118dc068edb672d40a19e61d9b74e936376844fea430b25baced1")
    );
    let wire = serde_json::to_string(&intent).expect("json");
    assert!(!wire.contains("stable-lineage-01"));
    assert!(!wire.contains("capability"));
    assert_eq!(
        fingerprint_origin_lineage("stable-lineage-01").expect("fingerprint"),
        intent.origin_lineage_fingerprint.expect("fingerprint")
    );
}

#[test]
fn exact_session_prefix_suffix_and_self_target_rules_are_enforced() {
    for invalid in [
        "claude#",
        "claude# source",
        "claude#source\nother",
        "grok#source",
        "acp:grok#",
    ] {
        let mut input = human_input();
        input.source_session_key = invalid.to_owned();
        assert!(
            normalize_collaboration_intent(&input, None).is_err(),
            "{invalid}"
        );
    }
    let mut self_target = human_input();
    self_target.target_engine = CollaborationEngineId::Claude;
    assert!(normalize_collaboration_intent(&self_target, None).is_err());
}

#[test]
fn mode_specific_transition_graph_has_no_cross_mode_mainline() {
    assert!(
        legal_collaboration_transitions(
            CollaborationState::PreparingTarget,
            CollaborationMode::Consult
        )
        .contains(&CollaborationState::RunningTarget)
    );
    assert!(
        !legal_collaboration_transitions(
            CollaborationState::PreparingTarget,
            CollaborationMode::Consult
        )
        .contains(&CollaborationState::ParkingSource)
    );
    assert!(
        legal_collaboration_transitions(
            CollaborationState::PreparingTarget,
            CollaborationMode::Handoff
        )
        .contains(&CollaborationState::ParkingSource)
    );
}

#[test]
fn pure_cas_rejects_stale_or_illegal_and_prunes_terminal_text() {
    let record = requested_record();
    let stale = transition_collaboration_record(
        &record,
        CollaborationTransitionRequest {
            expected_revision: 7,
            expected_state: CollaborationState::Requested,
            to: CollaborationState::Cancelled,
            maintenance_effect: None,
            at: AT.to_owned(),
            event_id: Some("event-stale".to_owned()),
            patch: CollaborationTransitionPatch::default(),
        },
    )
    .expect_err("stale");
    assert_eq!(stale.reason, ContractErrorReason::StaleRevision);

    let illegal = transition_collaboration_record(
        &record,
        CollaborationTransitionRequest {
            expected_revision: 0,
            expected_state: CollaborationState::Requested,
            to: CollaborationState::Completed,
            maintenance_effect: None,
            at: AT.to_owned(),
            event_id: Some("event-illegal".to_owned()),
            patch: CollaborationTransitionPatch::default(),
        },
    )
    .expect_err("illegal");
    assert_eq!(illegal.reason, ContractErrorReason::IllegalTransition);

    let cancelled = transition_collaboration_record(
        &record,
        CollaborationTransitionRequest {
            expected_revision: 0,
            expected_state: CollaborationState::Requested,
            to: CollaborationState::Cancelled,
            maintenance_effect: None,
            at: AT.to_owned(),
            event_id: Some("event-cancel".to_owned()),
            patch: CollaborationTransitionPatch::default(),
        },
    )
    .expect("cancelled")
    .record;
    assert_eq!(cancelled.revision, 1);
    assert_eq!(cancelled.terminal_at.as_deref(), Some(AT));
    assert!(cancelled.intent.objective.is_none());
    assert!(cancelled.validate().is_ok());
}

#[test]
fn legacy_javascript_safe_integer_ceiling_is_preserved() {
    let mut record = requested_record();
    record.revision = interface_collaboration::state::JS_MAX_SAFE_INTEGER + 1;
    assert!(record.validate().is_err());

    let mut record = requested_record();
    record.source.registry_revision = Some(interface_collaboration::state::JS_MAX_SAFE_INTEGER + 1);
    assert!(record.validate().is_err());
}

#[test]
fn requested_record_wire_names_are_stable() {
    let record = requested_record();
    let value = serde_json::to_value(record).expect("json");
    assert_eq!(value["version"], 1);
    assert_eq!(value["collaborationId"], "collaboration-1");
    assert_eq!(value["state"], "requested");
    assert_eq!(value["source"]["sessionKey"], "claude#source-1");
    assert_eq!(value["target"]["engine"], "codex");
    assert_eq!(value["effects"], json!({}));
    assert!(value.get("terminalAt").is_none());
}

#[test]
fn error_code_as_str_matches_serde_wire_tokens() {
    for code in COLLABORATION_ERROR_CODES {
        assert_eq!(
            serde_json::to_value(code).expect("serde").as_str(),
            Some(code.as_str())
        );
    }
    assert_eq!(
        CollaborationErrorCode::BrokerUnavailable.as_str(),
        "BROKER_UNAVAILABLE"
    );
}

#[test]
fn effect_inputs_required_matches_prepare_park_and_deliver_table() {
    assert_eq!(
        effect_inputs_required(CollaborationEffectKind::PrepareTarget),
        (true, true)
    );
    assert_eq!(
        effect_inputs_required(CollaborationEffectKind::ParkSource),
        (true, true)
    );
    assert_eq!(
        effect_inputs_required(CollaborationEffectKind::DeliverResult),
        (true, false)
    );
    assert_eq!(
        effect_inputs_required(CollaborationEffectKind::RunTarget),
        (false, false)
    );
    assert_eq!(
        effect_inputs_required(CollaborationEffectKind::TransferOwnership),
        (false, false)
    );
    assert_eq!(
        effect_inputs_required(CollaborationEffectKind::ActivateTarget),
        (false, false)
    );
}
