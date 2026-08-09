use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::routing::{get, post};
use axum::{Json, Router};
use buzz_project_view_pkg::v2::RoleLevel;
use buzz_project_view_pkg::v3::{
    DocumentReferenceMode, ProjectContextReference, ProjectObjectCommandV3, ProjectObjectRequestV3,
};
use buzz_project_view_pkg::ProjectViewObjectType;
use nostr::{Event, Keys};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;

use super::*;
use crate::app_state::build_app_state;
use crate::commands::project_view::PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION;
use crate::relay::SubmitEventResponse;

fn prepared_create_event() -> (Event, MutationTarget) {
    let prepared = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 4,
        object_type: ProjectViewObjectType::Plan,
        initial_role_level: None,
        acting_assignment_id: None,
        data: json!({
            "title": "Client",
            "description": "Human interface",
            "status": "active",
            "under_goal_id": null,
        }),
    })
    .expect("prepare v3 create");
    let event = prepared
        .builder
        .sign_with_keys(&Keys::generate())
        .expect("sign v3 create");
    (event, prepared.target)
}

#[test]
fn desktop_mutation_boundary_rejects_legacy_initialization_input() {
    let error = serde_json::from_value::<ProjectViewMutationInput>(json!({
        "operation": "initialize",
        "profile": {
            "name": "Legacy project",
            "positioning": "Legacy Desktop bootstrap",
            "purpose": "Must not be accepted",
            "problem": "Bypasses prepared v3 governance",
            "scope": "Desktop"
        },
        "goals": []
    }))
    .expect_err("Desktop initialization must not deserialize");
    assert!(error.to_string().contains("unknown variant `initialize`"));
}

#[test]
fn create_and_update_emit_only_schema_v3_commands() {
    let create = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 4,
        object_type: ProjectViewObjectType::Plan,
        initial_role_level: None,
        acting_assignment_id: None,
        data: json!({
            "title": "Client",
            "description": "Human interface",
            "status": "active",
            "under_goal_id": null,
        }),
    })
    .expect("prepare create");
    assert_eq!(create.expected_project_revision, 4);
    assert_eq!(create.target.object_type, ProjectViewObjectType::Plan);
    let event = create
        .builder
        .sign_with_keys(&Keys::generate())
        .expect("sign create");
    let command = ProjectObjectCommandV3::from_json(&event.content).expect("parse v3 create");
    assert_eq!(command.expected_project_revision, 4);
    assert!(matches!(command.request, ProjectObjectRequestV3::Create(_)));

    let object_id = Uuid::new_v4();
    let update = prepare_v3_mutation(ProjectViewMutationInput::Update {
        expected_project_revision: 5,
        object_type: ProjectViewObjectType::Issue,
        object_id,
        acting_assignment_id: None,
        patch: json!({"status": "resolved", "about": null}),
    })
    .expect("prepare update");
    assert_eq!(update.target.object_id, object_id);
    let event = update
        .builder
        .sign_with_keys(&Keys::generate())
        .expect("sign update");
    let command = ProjectObjectCommandV3::from_json(&event.content).expect("parse v3 update");
    assert!(matches!(command.request, ProjectObjectRequestV3::Update(_)));
}

#[test]
fn create_rejects_unknown_fields_before_signing() {
    let error = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 1,
        object_type: ProjectViewObjectType::Goal,
        initial_role_level: None,
        acting_assignment_id: None,
        data: json!({
            "title": "Ship",
            "desired_outcome": "Done",
            "directions": [],
            "raw_json_escape_hatch": true,
        }),
    })
    .expect_err("unknown field must fail");
    assert!(error.contains("unknown field"));
}

#[test]
fn role_create_preserves_signed_level_and_leader_assignment() {
    let assignment_id = Uuid::new_v4();
    let prepared = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 8,
        object_type: ProjectViewObjectType::Role,
        data: json!({
            "name": "Delivery leader",
            "purpose": "Govern member Roles",
            "responsibilities": [],
            "boundaries": [],
            "active": true,
        }),
        initial_role_level: Some(RoleLevel::Member),
        acting_assignment_id: Some(assignment_id),
    })
    .expect("prepare governed Role create");
    let event = prepared
        .builder
        .sign_with_keys(&Keys::generate())
        .expect("sign governed Role create");
    let command =
        ProjectObjectCommandV3::from_json(&event.content).expect("parse governed Role create");

    assert_eq!(command.initial_role_level, Some(RoleLevel::Member));
    assert_eq!(command.acting_assignment_id, Some(assignment_id));
}

#[test]
fn role_governance_fields_are_rejected_for_ordinary_objects() {
    let error = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 8,
        object_type: ProjectViewObjectType::Goal,
        data: json!({
            "title": "Ship",
            "desired_outcome": "Done",
            "directions": [],
        }),
        initial_role_level: Some(RoleLevel::Admin),
        acting_assignment_id: None,
    })
    .expect_err("ordinary object must reject Role level");
    assert!(error.contains("Role governance fields"));
}

#[test]
fn v3_resource_is_guide_only_and_rejects_the_legacy_locator_shape() {
    let guide_document_id = Uuid::new_v4();
    let prepared = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 8,
        object_type: ProjectViewObjectType::Resource,
        initial_role_level: None,
        acting_assignment_id: None,
        data: json!({
            "name": "Release console",
            "resource_kind": "internal-release-console-v7",
            "summary": "Coordinates releases",
            "guide_document_id": guide_document_id,
        }),
    })
    .expect("prepare v3 Resource");
    assert_eq!(
        prepared.summary_expectation,
        SummaryWriteExpectation::Set("Coordinates releases".to_owned())
    );
    let event = prepared
        .builder
        .sign_with_keys(&Keys::generate())
        .expect("sign v3 Resource");
    assert!(!event.content.contains("locator"));

    let error = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 8,
        object_type: ProjectViewObjectType::Resource,
        initial_role_level: None,
        acting_assignment_id: None,
        data: json!({
            "name": "Legacy",
            "resource_type": "repository",
            "locator": {"locator_type": "url", "value": "https://example.test"},
            "description": "must not cross the v3 boundary",
        }),
    })
    .expect_err("legacy Resource must fail closed");
    assert!(error.contains("invalid typed Project View v3 create data"));
}

#[test]
fn update_summary_wire_preserves_keep_set_and_clear() {
    let object_id = Uuid::new_v4();
    for (patch, expected) in [
        (
            json!({"status": "resolved"}),
            SummaryWriteExpectation::Unchanged,
        ),
        (
            json!({"summary": "Relevant when diagnosing release failures"}),
            SummaryWriteExpectation::Set("Relevant when diagnosing release failures".to_owned()),
        ),
        (json!({"summary": null}), SummaryWriteExpectation::Clear),
    ] {
        let prepared = prepare_v3_mutation(ProjectViewMutationInput::Update {
            expected_project_revision: 5,
            object_type: ProjectViewObjectType::Issue,
            object_id,
            acting_assignment_id: None,
            patch,
        })
        .expect("prepare summary update");
        assert_eq!(prepared.summary_expectation, expected);
    }
}

#[test]
fn v3_context_replacement_round_trips_only_closed_coordinates() {
    let object_id = Uuid::new_v4();
    let resource_id = Uuid::new_v4();
    let document_id = Uuid::new_v4();
    let references = vec![
        ProjectContextReference::Resource { resource_id },
        ProjectContextReference::Document {
            document_id,
            mode: DocumentReferenceMode::Live,
            document_revision: None,
        },
        ProjectContextReference::Document {
            document_id,
            mode: DocumentReferenceMode::Pinned,
            document_revision: Some(7),
        },
    ];
    let prepared = prepare_v3_mutation(ProjectViewMutationInput::Context {
        expected_project_revision: 11,
        object_type: ProjectViewObjectType::Role,
        object_id,
        acting_assignment_id: None,
        context_references: references.clone(),
    })
    .expect("prepare v3 Context replacement");
    let event = prepared
        .builder
        .sign_with_keys(&Keys::generate())
        .expect("sign v3 Context replacement");
    let command = ProjectObjectCommandV3::from_json(&event.content).expect("parse v3 command");
    let ProjectObjectRequestV3::Update(update) = command.request else {
        panic!("expected v3 update");
    };
    assert_eq!(update.object_id(), object_id);
    assert_eq!(update.context_references(), Some(references.as_slice()));
    assert!(!event.content.contains("content_markdown"));
}

#[test]
fn receipt_requires_the_canonical_response_prefix_and_v3_shape() {
    let (event, _) = prepared_create_event();
    let raw = SubmitEventResponse {
        event_id: event.id.to_hex(),
        accepted: true,
        message: json!({"project_revision": 5}).to_string(),
    };
    let error = parse_receipt(&raw, &event).expect_err("raw JSON must fail closed");
    assert!(error.contains("canonical `response:` prefix"));

    let legacy = SubmitEventResponse {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!("response:{}", json!({"project_revision": 5})),
    };
    let error = parse_receipt(&legacy, &event).expect_err("legacy receipt must fail closed");
    assert!(error.contains("invalid v3 mutation receipt"));
}

#[test]
fn v3_object_receipt_is_normalized_and_bound_to_the_signed_intent() {
    let (event, target) = prepared_create_event();
    let response = SubmitEventResponse {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            json!({
                "schema_version": 3,
                "operation": target.operation,
                "project_revision": 5,
                "objects": [{
                    "object_id": target.object_id,
                    "object_type": target.object_type.as_str(),
                    "object_revision": 1,
                    "deleted": false,
                }],
                "continuity_entities": [],
            })
        ),
    };
    let receipt = validate_receipt(
        parse_receipt(&response, &event).expect("parse v3 receipt"),
        target,
    )
    .expect("validate v3 receipt");
    assert_eq!(receipt.project_revision, 5);
    assert_eq!(receipt.object_id, Some(target.object_id));
    assert_eq!(receipt.object_revision, Some(1));

    let wrong_target = MutationTarget {
        operation: "update",
        ..target
    };
    let error = validate_receipt(
        parse_receipt(&response, &event).expect("parse v3 receipt again"),
        wrong_target,
    )
    .expect_err("receipt operation must match the request");
    assert!(error.contains("does not match"));
}

#[derive(Clone)]
struct LegacyRelayState {
    extension: &'static str,
    relay_pubkey: String,
    submissions: Arc<AtomicUsize>,
}

async fn legacy_info(AxumState(state): AxumState<LegacyRelayState>) -> Json<Value> {
    Json(json!({
        "supported_extensions": [state.extension],
        "self": state.relay_pubkey,
    }))
}

async fn count_submission(AxumState(state): AxumState<LegacyRelayState>) -> Json<Value> {
    state.submissions.fetch_add(1, Ordering::SeqCst);
    Json(json!({}))
}

#[tokio::test]
async fn legacy_and_bootstrap_only_relays_fail_before_any_submission() {
    for extension in [
        "buzz-project-view-v1",
        "buzz-project-view-v2",
        PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION,
    ] {
        let fixture = LegacyRelayState {
            extension,
            relay_pubkey: Keys::generate().public_key().to_hex(),
            submissions: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/info", get(legacy_info))
            .route("/events", post(count_submission))
            .with_state(fixture.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind legacy relay fixture");
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        let state = build_app_state();
        *state
            .relay_url_override
            .lock()
            .expect("lock Relay override") = Some(format!("http://{address}"));

        let error = execute_mutation(
            ProjectViewMutationInput::Create {
                expected_project_revision: 1,
                object_type: ProjectViewObjectType::Goal,
                data: json!({
                    "title": "Ship",
                    "desired_outcome": "Done",
                    "directions": [],
                }),
                initial_role_level: None,
                acting_assignment_id: None,
            },
            &state,
        )
        .await
        .expect_err("legacy relay must be unsupported");
        if extension == PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION {
            assert!(error.contains("requires an initialized and enabled Project View v3"));
        } else {
            assert!(error.contains("does not advertise Project View v3"));
        }
        assert_eq!(fixture.submissions.load(Ordering::SeqCst), 0);
    }
}
