use std::collections::BTreeMap;

use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_document::{
    reduce_document, CurrentDocument, DocumentCatalog, DocumentChangeContext,
    DocumentCommandRequest, DocumentError, DocumentOperation, DocumentRevision, DocumentState,
    ProjectDocumentCommand, MAX_SAFE_REVISION,
};
use chrono::{DateTime, Duration, Utc};
use proptest::prelude::*;
use uuid::Uuid;

fn fixed_actor() -> PublicKey {
    PublicKey::from_hex("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
        .expect("fixed actor")
}

fn project_id() -> CommunityId {
    CommunityId::from_uuid(Uuid::parse_str("3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77").expect("UUID"))
}

fn document_id(seed: u128) -> Uuid {
    let mut bytes = seed.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn time(second: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + second, 0).expect("timestamp")
}

fn change_id(seed: u8) -> EventId {
    EventId::from_hex(&format!("{seed:02x}").repeat(32)).expect("event ID")
}

fn create(id: Uuid, title: &str, body: &str) -> ProjectDocumentCommand {
    ProjectDocumentCommand::new(
        0,
        DocumentCommandRequest::Create {
            document_id: id,
            title: title.to_owned(),
            summary: None,
            content_markdown: body.to_owned(),
        },
    )
}

fn update(id: Uuid, revision: u64, title: &str, body: &str) -> ProjectDocumentCommand {
    ProjectDocumentCommand::new(
        revision,
        DocumentCommandRequest::Update {
            document_id: id,
            title: title.to_owned(),
            summary: None,
            content_markdown: body.to_owned(),
        },
    )
}

fn delete(id: Uuid, revision: u64) -> ProjectDocumentCommand {
    ProjectDocumentCommand::new(revision, DocumentCommandRequest::Delete { document_id: id })
}

#[test]
fn create_update_and_delete_form_full_immutable_revisions() {
    let actor = fixed_actor();
    let id = document_id(1);
    let catalog = DocumentCatalog::empty(project_id(), 1, time(0)).expect("empty catalog");

    let created = reduce_document(
        &catalog,
        None,
        &create(id, "Guide", "one"),
        DocumentChangeContext::new(actor, change_id(1), time(1)),
    )
    .expect("create");
    assert_eq!(created.current().document().current_revision(), 1);
    assert_eq!(created.current().document().state(), DocumentState::Active);
    assert_eq!(created.catalog().catalog_revision(), 1);
    assert_eq!(created.catalog().active_document_count(), 1);
    assert_eq!(created.receipt().operation, DocumentOperation::Create);
    assert_eq!(
        created
            .current()
            .revision()
            .snapshot()
            .map(|value| value.content_markdown.as_str()),
        Some("one")
    );

    let updated = reduce_document(
        created.catalog(),
        Some(created.current()),
        &update(id, 1, "Guide v2", "two"),
        DocumentChangeContext::new(actor, change_id(2), time(2)),
    )
    .expect("update");
    assert_eq!(updated.current().document().current_revision(), 2);
    assert_eq!(updated.catalog().catalog_revision(), 2);
    assert_eq!(updated.catalog().active_document_count(), 1);
    assert_eq!(
        updated
            .current()
            .revision()
            .snapshot()
            .map(|value| value.content_markdown.as_str()),
        Some("two")
    );
    assert_eq!(
        created
            .current()
            .revision()
            .snapshot()
            .map(|value| value.content_markdown.as_str()),
        Some("one"),
        "the old full snapshot remains immutable"
    );

    let deleted = reduce_document(
        updated.catalog(),
        Some(updated.current()),
        &delete(id, 2),
        DocumentChangeContext::new(actor, change_id(3), time(3)),
    )
    .expect("delete");
    assert_eq!(deleted.current().document().current_revision(), 3);
    assert_eq!(deleted.current().document().state(), DocumentState::Deleted);
    assert!(matches!(
        deleted.current().revision(),
        DocumentRevision::Deleted { .. }
    ));
    assert_eq!(deleted.current().revision().snapshot(), None);
    assert_eq!(deleted.catalog().active_document_count(), 0);
}

#[test]
fn conflicts_noop_references_and_identity_reuse_fail_without_output() {
    let actor = fixed_actor();
    let id = document_id(2);
    let catalog = DocumentCatalog::empty(project_id(), 1, time(0)).expect("catalog");
    let created = reduce_document(
        &catalog,
        None,
        &create(id, "Guide", "body"),
        DocumentChangeContext::new(actor, change_id(1), time(1)),
    )
    .expect("create");

    assert!(matches!(
        reduce_document(
            created.catalog(),
            Some(created.current()),
            &update(id, 7, "changed", "changed"),
            DocumentChangeContext::new(actor, change_id(2), time(2)),
        ),
        Err(DocumentError::RevisionConflict {
            expected: 7,
            actual: Some(1)
        })
    ));
    assert!(matches!(
        reduce_document(
            created.catalog(),
            Some(created.current()),
            &update(id, 1, "Guide", "body"),
            DocumentChangeContext::new(actor, change_id(3), time(2)),
        ),
        Err(DocumentError::NoChange)
    ));
    assert!(matches!(
        reduce_document(
            created.catalog(),
            Some(created.current()),
            &delete(id, 1),
            DocumentChangeContext::new(actor, change_id(4), time(2))
                .with_deletion_blocked(true),
        ),
        Err(DocumentError::StillReferenced { document_id }) if document_id == id
    ));

    let deleted = reduce_document(
        created.catalog(),
        Some(created.current()),
        &delete(id, 1),
        DocumentChangeContext::new(actor, change_id(5), time(2)),
    )
    .expect("delete");
    assert!(matches!(
        reduce_document(
            deleted.catalog(),
            Some(deleted.current()),
            &create(id, "reused", "body"),
            DocumentChangeContext::new(actor, change_id(6), time(3)),
        ),
        Err(DocumentError::DocumentIdAlreadyExists { document_id }) if document_id == id
    ));
    assert!(matches!(
        reduce_document(
            deleted.catalog(),
            Some(deleted.current()),
            &delete(id, 2),
            DocumentChangeContext::new(actor, change_id(7), time(3)),
        ),
        Err(DocumentError::DocumentDeleted { document_id }) if document_id == id
    ));
}

#[test]
fn document_revisions_are_independent_while_catalog_observation_advances() {
    let actor = fixed_actor();
    let first_id = document_id(10);
    let second_id = document_id(11);
    let catalog = DocumentCatalog::empty(project_id(), 1, time(0)).expect("catalog");
    let first = reduce_document(
        &catalog,
        None,
        &create(first_id, "First", "one"),
        DocumentChangeContext::new(actor, change_id(1), time(1)),
    )
    .expect("first create");
    let second = reduce_document(
        first.catalog(),
        None,
        &create(second_id, "Second", "two"),
        DocumentChangeContext::new(actor, change_id(2), time(2)),
    )
    .expect("second create");
    let first_update = reduce_document(
        second.catalog(),
        Some(first.current()),
        &update(first_id, 1, "First v2", "one plus"),
        DocumentChangeContext::new(actor, change_id(3), time(3)),
    )
    .expect("first update");

    assert_eq!(first_update.current().document().current_revision(), 2);
    assert_eq!(second.current().document().current_revision(), 1);
    assert_eq!(first_update.catalog().catalog_revision(), 3);
    assert_eq!(first_update.catalog().active_document_count(), 2);
}

#[test]
fn revision_overflow_and_non_monotonic_time_fail_closed() {
    let actor = fixed_actor();
    let id = document_id(20);
    let catalog =
        DocumentCatalog::from_snapshot(project_id(), MAX_SAFE_REVISION, 1, 1, time(0), time(1))
            .expect("max catalog");
    let document = buzz_project_document::ProjectDocument::from_snapshot(
        id,
        MAX_SAFE_REVISION,
        DocumentState::Active,
        buzz_project_document::DocumentAttribution {
            at: time(0),
            by: actor,
        },
        buzz_project_document::DocumentAttribution {
            at: time(1),
            by: actor,
        },
    )
    .expect("max document");
    let current = CurrentDocument::new(
        document,
        DocumentRevision::Active {
            schema_version: 1,
            document_id: id,
            document_revision: MAX_SAFE_REVISION,
            snapshot: buzz_project_document::DocumentSnapshot {
                title: "Max".to_owned(),
                summary: None,
                content_markdown: "body".to_owned(),
            },
            actor,
            canonical_at: time(1),
        },
    )
    .expect("max current");
    assert!(matches!(
        reduce_document(
            &catalog,
            Some(&current),
            &update(id, MAX_SAFE_REVISION, "Next", "body"),
            DocumentChangeContext::new(actor, change_id(1), time(2)),
        ),
        Err(DocumentError::RevisionExhausted)
    ));

    let ordinary_catalog = DocumentCatalog::empty(project_id(), 1, time(10)).expect("catalog");
    assert!(matches!(
        reduce_document(
            &ordinary_catalog,
            None,
            &create(document_id(21), "Guide", "body"),
            DocumentChangeContext::new(actor, change_id(2), time(10)),
        ),
        Err(DocumentError::InvalidCanonicalState { .. })
    ));
}

#[test]
fn reduction_and_projection_plan_are_deterministic() {
    let catalog = DocumentCatalog::empty(project_id(), 1, time(0)).expect("catalog");
    let command = create(document_id(30), "Guide", "body");
    let context = DocumentChangeContext::new(fixed_actor(), change_id(1), time(1));
    let first = reduce_document(&catalog, None, &command, context).expect("first reduction");
    let second = reduce_document(&catalog, None, &command, context).expect("second reduction");
    assert_eq!(first, second);
    first.validate().expect("valid transition");
}

proptest! {
    #[test]
    fn arbitrary_accepted_sequences_preserve_catalog_and_current_invariants(
        steps in prop::collection::vec((0u8..4, any::<bool>(), 0u16..1000), 1..=96)
    ) {
        let actor = fixed_actor();
        let mut catalog = DocumentCatalog::empty(project_id(), 1, time(0)).expect("catalog");
        let mut documents: BTreeMap<Uuid, CurrentDocument> = BTreeMap::new();
        let mut accepted = 0_i64;

        for (slot, prefer_update, version) in steps {
            let id = document_id(u128::from(slot) + 100);
            let command = match documents.get(&id) {
                None => create(id, &format!("Document {slot}"), &format!("body-{version}")),
                Some(current) if current.document().state() == DocumentState::Active => {
                    if prefer_update {
                        update(
                            id,
                            current.document().current_revision(),
                            &format!("Document {slot} revision {version}"),
                            &format!("body-{version}-{}", accepted + 1),
                        )
                    } else {
                        delete(id, current.document().current_revision())
                    }
                }
                Some(_) => create(id, "cannot reuse", "body"),
            };
            let before_catalog = catalog.clone();
            let before_documents = documents.clone();
            let current = documents.get(&id);
            let at = time(1) + Duration::seconds(accepted + 1);
            let seed = u8::try_from((accepted % 250) + 1).expect("bounded seed");
            match reduce_document(
                &catalog,
                current,
                &command,
                DocumentChangeContext::new(actor, change_id(seed), at),
            ) {
                Ok(transition) => {
                    transition.validate().expect("accepted transition invariant");
                    prop_assert_eq!(
                        transition.catalog().catalog_revision(),
                        catalog.catalog_revision() + 1
                    );
                    catalog = transition.catalog().clone();
                    documents.insert(id, transition.current().clone());
                    accepted += 1;
                }
                Err(_) => {
                    prop_assert_eq!(&catalog, &before_catalog);
                    prop_assert_eq!(&documents, &before_documents);
                }
            }

            catalog.validate().expect("catalog invariant");
            for current in documents.values() {
                current.validate().expect("current invariant");
            }
            let active = documents
                .values()
                .filter(|current| current.document().state() == DocumentState::Active)
                .count() as u64;
            prop_assert_eq!(catalog.active_document_count(), active);
        }
    }
}
