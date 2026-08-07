use buzz_core::{EventId, RuntimeFence};
use buzz_project_context::{
    canonicalize_coordinates, EdgeKey, ProjectContextBindingProjection, ProjectContextBindingState,
    ProjectContextCommand, ProjectContextCoordinate, ProjectContextError, ProjectContextOperation,
    ProjectContextProjectionType, MAX_COMMAND_CONTENT_BYTES, MAX_COMMAND_JSON_DEPTH,
    MAX_PROJECTION_CONTENT_BYTES, MAX_SAFE_REVISION, MIN_EDGE_COORDINATES,
    PROJECT_CONTEXT_SCHEMA_VERSION,
};
use buzz_project_view::ProjectViewObjectType;
use serde_json::Value;
use uuid::Uuid;

fn uuid(seed: u128) -> Uuid {
    let mut bytes = seed.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn coordinates() -> Vec<ProjectContextCoordinate> {
    canonicalize_coordinates(vec![
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Requirement,
            object_id: uuid(1),
        },
        ProjectContextCoordinate::Document {
            document_id: uuid(2),
        },
    ])
    .expect("coordinates")
}

#[test]
fn frozen_limits_and_closed_json_are_enforced() {
    assert_eq!(MIN_EDGE_COORDINATES, 2);
    assert_eq!(MAX_COMMAND_CONTENT_BYTES, 65_536);
    assert_eq!(MAX_PROJECTION_CONTENT_BYTES, 65_536);
    assert_eq!(MAX_COMMAND_JSON_DEPTH, 16);
    assert_eq!(MAX_SAFE_REVISION, 9_007_199_254_740_991);

    assert!(matches!(
        ProjectContextCommand::from_json(&" ".repeat(MAX_COMMAND_CONTENT_BYTES + 1)),
        Err(ProjectContextError::ContentTooLarge { .. })
    ));
    let too_deep = format!(
        "{}0{}",
        "[".repeat(MAX_COMMAND_JSON_DEPTH),
        "]".repeat(MAX_COMMAND_JSON_DEPTH)
    );
    assert!(matches!(
        ProjectContextCommand::from_json(&too_deep),
        Err(ProjectContextError::JsonTooDeep { .. })
    ));

    let command =
        ProjectContextCommand::new(0, ProjectContextOperation::Attach, coordinates(), uuid(3))
            .expect("command");
    let mut value = serde_json::to_value(&command).expect("value");
    value["unexpected"] = Value::Bool(true);
    assert!(ProjectContextCommand::from_json(&value.to_string()).is_err());
    let mut value = serde_json::to_value(&command).expect("value");
    value["request"]["coordinates"][0]["unexpected"] = Value::Bool(true);
    assert!(ProjectContextCommand::from_json(&value.to_string()).is_err());

    let mut value = serde_json::to_value(&command).expect("value");
    value["request"]["coordinates"][0]["coordinate_type"] =
        Value::String("future_coordinate".to_owned());
    assert!(ProjectContextCommand::from_json(&value.to_string()).is_err());

    let mut value = serde_json::to_value(&command).expect("value");
    value["request"]["type"] = Value::String("move".to_owned());
    assert!(ProjectContextCommand::from_json(&value.to_string()).is_err());

    assert!(canonicalize_coordinates(vec![
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Requirement,
            object_id: Uuid::nil(),
        },
        ProjectContextCoordinate::Document {
            document_id: uuid(2),
        },
    ])
    .is_err());
    assert!(ProjectContextCommand::new(
        0,
        ProjectContextOperation::Attach,
        coordinates(),
        Uuid::nil(),
    )
    .is_err());
}

#[test]
fn option_presence_runtime_pair_and_safe_revision_are_strict() {
    let command =
        ProjectContextCommand::new(0, ProjectContextOperation::Attach, coordinates(), uuid(3))
            .expect("command")
            .with_runtime_fence(
                uuid(4),
                RuntimeFence {
                    runtime_id: uuid(5),
                    runtime_epoch: 1,
                },
            );
    command.validate_for_submission().expect("paired fence");

    let mut null_fence = serde_json::to_value(&command).expect("value");
    null_fence["runtime_fence"] = Value::Null;
    assert!(ProjectContextCommand::from_json(&null_fence.to_string()).is_err());

    let mut missing_fence = serde_json::to_value(&command).expect("value");
    missing_fence
        .as_object_mut()
        .expect("object")
        .remove("runtime_fence");
    assert!(ProjectContextCommand::from_json(&missing_fence.to_string()).is_err());

    let mut over = command;
    over.expected_context_revision = MAX_SAFE_REVISION + 1;
    assert!(over.validate_for_submission().is_err());
}

#[test]
fn project_profile_coordinate_must_name_the_host_project() {
    let project = uuid(10);
    let command = ProjectContextCommand::new(
        0,
        ProjectContextOperation::Attach,
        vec![
            ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::ProjectProfile,
                object_id: uuid(11),
            },
            ProjectContextCoordinate::Document {
                document_id: uuid(12),
            },
        ],
        uuid(13),
    )
    .expect("wire-valid command");
    assert!(matches!(
        command.validate_for_project(project),
        Err(ProjectContextError::InvalidCoordinate { .. })
    ));
}

#[test]
fn a_legal_command_can_still_fail_the_derived_projection_content_limit() {
    let project = Uuid::parse_str("3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77").expect("project UUID");
    let source = EventId::from_hex(&"11".repeat(32)).expect("event ID");
    let updated_at = chrono::DateTime::from_timestamp(1_800_000_000, 0).expect("timestamp");
    let mut boundary = None;

    for count in 2u128..=900 {
        let coordinates = canonicalize_coordinates(
            (1..=count)
                .map(|seed| ProjectContextCoordinate::ProjectViewObject {
                    object_type: ProjectViewObjectType::Requirement,
                    object_id: uuid(seed),
                })
                .collect(),
        )
        .expect("canonical coordinates");
        let command = ProjectContextCommand::new(
            0,
            ProjectContextOperation::Attach,
            coordinates.clone(),
            uuid(10_000),
        )
        .expect("command shape");
        let raw_command = serde_json::to_string(&command).expect("command JSON");
        if raw_command.len() > MAX_COMMAND_CONTENT_BYTES {
            break;
        }
        ProjectContextCommand::from_json(&raw_command).expect("legal command content");
        let projection = ProjectContextBindingProjection {
            schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
            projection_type: ProjectContextProjectionType::ContextEdgeBinding,
            project_id: project,
            projection_generation: 1,
            context_revision: 1,
            edge_key: EdgeKey::derive(project, &coordinates).expect("edge key"),
            coordinates,
            context_document_id: uuid(10_000),
            state: ProjectContextBindingState::Active,
            source_event_id: source,
            updated_at,
        };
        let projection_bytes = serde_json::to_vec(&projection)
            .expect("projection JSON")
            .len();
        if projection_bytes > MAX_PROJECTION_CONTENT_BYTES {
            boundary = Some(projection);
            break;
        }
    }

    let projection = boundary.expect("a content-overhead boundary must exist");
    assert!(matches!(
        projection.validate(),
        Err(ProjectContextError::ProjectionTooLarge { .. })
    ));
}
