use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_context::{
    canonicalize_coordinates, reduce_project_context, EdgeKey, ProjectContextBindingState,
    ProjectContextCatalog, ProjectContextChangeContext, ProjectContextCommand,
    ProjectContextCoordinate, ProjectContextError, ProjectContextOperation,
};
use buzz_project_view::ProjectViewObjectType;
use chrono::{DateTime, Utc};
use proptest::prelude::*;
use uuid::Uuid;

const PROJECT: &str = "3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77";
const PROJECT_TWO: &str = "825a0671-d1b8-4472-9e7e-405c186d1575";

fn uuid(seed: u128) -> Uuid {
    let mut bytes = seed.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn project() -> Uuid {
    Uuid::parse_str(PROJECT).expect("project UUID")
}

fn actor() -> PublicKey {
    PublicKey::from_hex("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
        .expect("fixed actor")
}

fn change(seed: u8) -> EventId {
    EventId::from_hex(&format!("{seed:02x}").repeat(32)).expect("event ID")
}

fn time(second: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + second, 0).expect("timestamp")
}

fn requirement(id: Uuid) -> ProjectContextCoordinate {
    ProjectContextCoordinate::ProjectViewObject {
        object_type: ProjectViewObjectType::Requirement,
        object_id: id,
    }
}

fn resource(id: Uuid) -> ProjectContextCoordinate {
    ProjectContextCoordinate::ProjectViewObject {
        object_type: ProjectViewObjectType::Resource,
        object_id: id,
    }
}

fn document(id: Uuid) -> ProjectContextCoordinate {
    ProjectContextCoordinate::Document { document_id: id }
}

fn meeting(id: Uuid) -> ProjectContextCoordinate {
    ProjectContextCoordinate::Meeting { meeting_id: id }
}

fn coordinates() -> Vec<ProjectContextCoordinate> {
    canonicalize_coordinates(vec![
        document(uuid(3)),
        requirement(uuid(1)),
        resource(uuid(2)),
    ])
    .expect("canonical coordinates")
}

#[test]
fn explicit_order_appends_meeting_after_existing_coordinate_families() {
    let values = canonicalize_coordinates(vec![
        meeting(uuid(1)),
        document(uuid(1)),
        resource(uuid(1)),
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Goal,
            object_id: uuid(9),
        },
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Goal,
            object_id: uuid(2),
        },
        requirement(uuid(1)),
    ])
    .expect("canonical set");

    assert!(matches!(
        values[0],
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Goal,
            object_id,
        } if object_id == uuid(2)
    ));
    assert!(matches!(
        values[1],
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Goal,
            object_id,
        } if object_id == uuid(9)
    ));
    assert!(matches!(
        values[2],
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Requirement,
            ..
        }
    ));
    assert!(matches!(
        values[3],
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Resource,
            ..
        }
    ));
    assert!(matches!(
        values[4],
        ProjectContextCoordinate::Document { .. }
    ));
    assert!(matches!(
        values[5],
        ProjectContextCoordinate::Meeting { meeting_id } if meeting_id == uuid(1)
    ));
}

#[test]
fn meeting_coordinate_has_stable_tag_and_mixed_edge_identity() {
    let meeting_id = uuid(41);
    let coordinate = meeting(meeting_id);
    assert_eq!(
        coordinate.tag_value(project()),
        format!("meeting:{}:{meeting_id}", project())
    );

    let existing_only = canonicalize_coordinates(vec![requirement(uuid(1)), resource(uuid(2))])
        .expect("existing-only coordinates");
    assert_eq!(
        EdgeKey::derive(project(), &existing_only)
            .expect("legacy-stable key")
            .to_string(),
        "95998c5b78b6fa4efda616f85841aa001fce244775aa2ea4c5ae5ab9ec566c34"
    );

    let mixed = canonicalize_coordinates(vec![meeting(meeting_id), document(uuid(3))])
        .expect("mixed coordinates");
    assert_ne!(
        EdgeKey::derive(project(), &existing_only).expect("existing key"),
        EdgeKey::derive(project(), &mixed).expect("mixed key")
    );
}

#[test]
fn edge_identity_distinguishes_scope_and_project() {
    let a = requirement(uuid(1));
    let b = requirement(uuid(2));
    let c = resource(uuid(3));
    let ab = canonicalize_coordinates(vec![a.clone(), b.clone()]).expect("AB");
    let abc = canonicalize_coordinates(vec![a, b, c]).expect("ABC");
    let project_two = Uuid::parse_str(PROJECT_TWO).expect("second project UUID");

    assert_ne!(
        EdgeKey::derive(project(), &ab).expect("AB key"),
        EdgeKey::derive(project(), &abc).expect("ABC key")
    );
    assert_ne!(
        EdgeKey::derive(project(), &ab).expect("first project key"),
        EdgeKey::derive(project_two, &ab).expect("second project key")
    );
}

#[test]
fn duplicate_too_small_and_noncanonical_coordinate_sets_fail() {
    let a = requirement(uuid(1));
    assert!(matches!(
        canonicalize_coordinates(vec![a.clone()]),
        Err(ProjectContextError::TooFewCoordinates { .. })
    ));
    assert!(matches!(
        canonicalize_coordinates(vec![a.clone(), a]),
        Err(ProjectContextError::DuplicateCoordinate)
    ));

    let value = serde_json::json!({
        "schema_version": 2,
        "expected_context_revision": 0,
        "request": {
            "type": "attach",
            "coordinates": [
                {"coordinate_type":"document", "document_id": uuid(2)},
                {"coordinate_type":"project_view_object", "object_type":"requirement", "object_id": uuid(1)}
            ],
            "context_document_id": uuid(3)
        }
    });
    assert!(matches!(
        ProjectContextCommand::from_json(&value.to_string()),
        Err(ProjectContextError::NonCanonicalCoordinates)
    ));
}

#[test]
fn one_document_can_bind_only_one_edge_and_edge_lifetime_follows_membership() {
    let first_document = uuid(100);
    let second_document = uuid(101);
    let edge_coordinates = coordinates();
    let catalog = ProjectContextCatalog::empty(CommunityId::from_uuid(project()), 1, time(0))
        .expect("empty catalog");

    let first_command = ProjectContextCommand::new(
        0,
        ProjectContextOperation::Attach,
        edge_coordinates.clone(),
        first_document,
    )
    .expect("first attach command");
    let first = reduce_project_context(
        &catalog,
        None,
        None,
        &first_command,
        ProjectContextChangeContext::active(actor(), change(1), time(1)),
    )
    .expect("first attach");
    assert_eq!(first.catalog().active_edge_count(), 1);
    assert_eq!(first.catalog().bound_document_count(), 1);
    assert_eq!(
        first.edge().expect("active edge").context_document_ids(),
        &[first_document]
    );
    assert_eq!(first.binding().state, ProjectContextBindingState::Active);

    let second_command = ProjectContextCommand::new(
        1,
        ProjectContextOperation::Attach,
        edge_coordinates.clone(),
        second_document,
    )
    .expect("second attach command");
    let second = reduce_project_context(
        first.catalog(),
        first.edge(),
        None,
        &second_command,
        ProjectContextChangeContext::active(actor(), change(2), time(2)),
    )
    .expect("second attach");
    assert_eq!(second.catalog().active_edge_count(), 1);
    assert_eq!(second.catalog().bound_document_count(), 2);
    assert_eq!(
        second
            .edge()
            .expect("active edge")
            .context_document_ids()
            .len(),
        2
    );

    assert!(matches!(
        reduce_project_context(
            second.catalog(),
            second.edge(),
            Some(second.edge().expect("edge").key()),
            &ProjectContextCommand::new(
                2,
                ProjectContextOperation::Attach,
                edge_coordinates.clone(),
                second_document,
            )
            .expect("duplicate attach command"),
            ProjectContextChangeContext::active(actor(), change(3), time(3)),
        ),
        Err(ProjectContextError::NoChange)
    ));

    let other_coordinates =
        canonicalize_coordinates(vec![requirement(uuid(1)), requirement(uuid(200))])
            .expect("other edge");
    let other_command = ProjectContextCommand::new(
        2,
        ProjectContextOperation::Attach,
        other_coordinates,
        second_document,
    )
    .expect("other attach command");
    assert!(matches!(
        reduce_project_context(
            second.catalog(),
            None,
            Some(second.edge().expect("edge").key()),
            &other_command,
            ProjectContextChangeContext::active(actor(), change(4), time(3)),
        ),
        Err(ProjectContextError::DocumentAlreadyBound { .. })
    ));

    let detach_first = ProjectContextCommand::new(
        2,
        ProjectContextOperation::Detach,
        edge_coordinates.clone(),
        first_document,
    )
    .expect("detach first command");
    let after_first_detach = reduce_project_context(
        second.catalog(),
        second.edge(),
        Some(second.edge().expect("edge").key()),
        &detach_first,
        ProjectContextChangeContext::active(actor(), change(5), time(3))
            .with_coordinates_active(false)
            .with_context_document_active(false),
    )
    .expect("detach permits tombstoned coordinates and target");
    assert_eq!(after_first_detach.catalog().active_edge_count(), 1);
    assert_eq!(after_first_detach.catalog().bound_document_count(), 1);
    assert_eq!(
        after_first_detach.binding().state,
        ProjectContextBindingState::Deleted
    );

    let expected_edge_key = second.edge().expect("edge").key();
    let detach_last = ProjectContextCommand::new(
        3,
        ProjectContextOperation::Detach,
        edge_coordinates.clone(),
        second_document,
    )
    .expect("detach last command");
    let after_last_detach = reduce_project_context(
        after_first_detach.catalog(),
        after_first_detach.edge(),
        Some(after_first_detach.edge().expect("edge").key()),
        &detach_last,
        ProjectContextChangeContext::active(actor(), change(6), time(4)),
    )
    .expect("detach last");
    assert!(after_last_detach.edge().is_none());
    assert_eq!(after_last_detach.catalog().active_edge_count(), 0);
    assert_eq!(after_last_detach.catalog().bound_document_count(), 0);
    assert_eq!(after_last_detach.receipt().edge_document_count, 0);
    assert_eq!(
        after_last_detach.receipt().edge_state,
        ProjectContextBindingState::Deleted
    );

    let recreate = ProjectContextCommand::new(
        4,
        ProjectContextOperation::Attach,
        edge_coordinates,
        uuid(102),
    )
    .expect("recreate command");
    let recreated = reduce_project_context(
        after_last_detach.catalog(),
        None,
        None,
        &recreate,
        ProjectContextChangeContext::active(actor(), change(7), time(5)),
    )
    .expect("recreate the deleted exact set");
    assert_eq!(
        recreated.edge().expect("recreated edge").key(),
        expected_edge_key
    );
}

#[test]
fn attach_requires_live_inputs_and_exact_revision() {
    let catalog = ProjectContextCatalog::empty(CommunityId::from_uuid(project()), 1, time(0))
        .expect("catalog");
    let command =
        ProjectContextCommand::new(0, ProjectContextOperation::Attach, coordinates(), uuid(300))
            .expect("command");
    assert!(matches!(
        reduce_project_context(
            &catalog,
            None,
            None,
            &command,
            ProjectContextChangeContext::active(actor(), change(1), time(1))
                .with_coordinates_active(false),
        ),
        Err(ProjectContextError::InactiveCoordinate)
    ));
    assert!(matches!(
        reduce_project_context(
            &catalog,
            None,
            None,
            &command,
            ProjectContextChangeContext::active(actor(), change(1), time(1))
                .with_context_document_active(false),
        ),
        Err(ProjectContextError::InactiveContextDocument { .. })
    ));
    let stale =
        ProjectContextCommand::new(1, ProjectContextOperation::Attach, coordinates(), uuid(300))
            .expect("stale command shape");
    assert!(matches!(
        reduce_project_context(
            &catalog,
            None,
            None,
            &stale,
            ProjectContextChangeContext::active(actor(), change(1), time(1)),
        ),
        Err(ProjectContextError::RevisionConflict {
            expected: 1,
            actual: 0
        })
    ));
}

#[test]
fn catalog_identity_counts_generation_and_revision_space_are_closed() {
    assert!(ProjectContextCatalog::empty(CommunityId::from_uuid(Uuid::nil()), 1, time(0)).is_err());
    assert!(ProjectContextCatalog::from_snapshot(
        CommunityId::from_uuid(project()),
        0,
        1,
        1,
        1,
        time(0),
        time(0),
    )
    .is_err());
    assert!(ProjectContextCatalog::from_snapshot(
        CommunityId::from_uuid(project()),
        1,
        2,
        1,
        1,
        time(0),
        time(1),
    )
    .is_err());
    assert!(ProjectContextCatalog::empty(CommunityId::from_uuid(project()), 0, time(0)).is_err());

    let exhausted = ProjectContextCatalog::from_snapshot(
        CommunityId::from_uuid(project()),
        buzz_project_context::MAX_SAFE_REVISION,
        0,
        0,
        1,
        time(0),
        time(1),
    )
    .expect("max-revision empty catalog");
    let command = ProjectContextCommand::new(
        buzz_project_context::MAX_SAFE_REVISION,
        ProjectContextOperation::Attach,
        coordinates(),
        uuid(400),
    )
    .expect("max-revision command");
    assert!(matches!(
        reduce_project_context(
            &exhausted,
            None,
            None,
            &command,
            ProjectContextChangeContext::active(actor(), change(8), time(2)),
        ),
        Err(ProjectContextError::RevisionExhausted)
    ));
}

proptest! {
    #[test]
    fn every_permutation_of_a_coordinate_set_has_one_edge_key(
        left in any::<u128>(),
        right in any::<u128>(),
        document_seed in any::<u128>(),
        permutation in 0u8..6,
    ) {
        let base = vec![
            requirement(uuid(left)),
            resource(uuid(right)),
            document(uuid(document_seed)),
        ];
        let order = match permutation {
            0 => [0, 1, 2],
            1 => [0, 2, 1],
            2 => [1, 0, 2],
            3 => [1, 2, 0],
            4 => [2, 0, 1],
            _ => [2, 1, 0],
        };
        let permuted = order.into_iter().map(|index| base[index].clone()).collect();
        let canonical = canonicalize_coordinates(base).expect("base canonicalization");
        let candidate = canonicalize_coordinates(permuted).expect("permuted canonicalization");
        prop_assert_eq!(
            EdgeKey::derive(project(), &canonical).expect("base key"),
            EdgeKey::derive(project(), &candidate).expect("candidate key")
        );
    }
}
