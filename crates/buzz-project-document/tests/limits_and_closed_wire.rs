use buzz_core::RuntimeFence;
use buzz_project_document::{
    DocumentCommandRequest, DocumentError, DocumentHeadProjection, DocumentMetaProjection,
    ProjectDocumentCommand, MAX_COMMAND_CONTENT_BYTES, MAX_COMMAND_JSON_DEPTH,
    MAX_CONTENT_MARKDOWN_BYTES, MAX_SAFE_REVISION, MAX_SUMMARY_BYTES, MAX_TITLE_BYTES,
};
use serde_json::{json, Value};
use uuid::Uuid;

const DOCUMENT: &str = "9c23f672-a397-42d1-b933-104ba2674f26";
const ASSIGNMENT: &str = "151f2347-7d24-41a0-ab0d-f272e84fcf88";
const RUNTIME: &str = "74ad5e95-903b-4488-ac19-d95a73fa62d4";

#[test]
fn frozen_limit_values_and_inclusive_boundaries_are_enforced() {
    assert_eq!(MAX_COMMAND_CONTENT_BYTES, 65_536);
    assert_eq!(MAX_COMMAND_JSON_DEPTH, 16);
    assert_eq!(MAX_TITLE_BYTES, 256);
    assert_eq!(MAX_SUMMARY_BYTES, 4_096);
    assert_eq!(MAX_CONTENT_MARKDOWN_BYTES, 49_152);
    assert_eq!(MAX_SAFE_REVISION, 9_007_199_254_740_991);

    active_command("a".repeat(MAX_TITLE_BYTES), None, String::new())
        .validate_for_submission()
        .expect("title boundary");
    active_command("a".repeat(MAX_TITLE_BYTES + 1), None, String::new())
        .validate_for_submission()
        .expect_err("title over boundary");
    active_command(
        "title".to_owned(),
        Some("s".repeat(MAX_SUMMARY_BYTES)),
        String::new(),
    )
    .validate_for_submission()
    .expect("summary boundary");
    active_command(
        "title".to_owned(),
        Some("s".repeat(MAX_SUMMARY_BYTES + 1)),
        String::new(),
    )
    .validate_for_submission()
    .expect_err("summary over boundary");
    active_command(
        "title".to_owned(),
        None,
        "m".repeat(MAX_CONTENT_MARKDOWN_BYTES),
    )
    .validate_for_submission()
    .expect("Markdown boundary");
    active_command(
        "title".to_owned(),
        None,
        "m".repeat(MAX_CONTENT_MARKDOWN_BYTES + 1),
    )
    .validate_for_submission()
    .expect_err("Markdown over boundary");

    assert!(matches!(
        ProjectDocumentCommand::from_json(&" ".repeat(MAX_COMMAND_CONTENT_BYTES + 1)),
        Err(DocumentError::ContentTooLarge { .. })
    ));
    let too_deep = format!(
        "{}0{}",
        "[".repeat(MAX_COMMAND_JSON_DEPTH),
        "]".repeat(MAX_COMMAND_JSON_DEPTH)
    );
    assert!(matches!(
        ProjectDocumentCommand::from_json(&too_deep),
        Err(DocumentError::JsonTooDeep { .. })
    ));
}

#[test]
fn optional_values_and_runtime_fence_are_presence_aware() {
    let document = Uuid::parse_str(DOCUMENT).expect("document UUID");
    let assignment = Uuid::parse_str(ASSIGNMENT).expect("Assignment UUID");
    let runtime = Uuid::parse_str(RUNTIME).expect("Runtime UUID");
    let canonical = ProjectDocumentCommand::new(
        1,
        DocumentCommandRequest::Update {
            document_id: document,
            title: "title".to_owned(),
            summary: None,
            content_markdown: String::new(),
        },
    )
    .with_runtime_fence(
        assignment,
        RuntimeFence {
            runtime_id: runtime,
            runtime_epoch: 1,
        },
    );
    canonical
        .validate_for_submission()
        .expect("paired runtime fence");

    let mut value = serde_json::to_value(&canonical).expect("command JSON");
    value["runtime_fence"] = Value::Null;
    assert!(ProjectDocumentCommand::from_json(&value.to_string()).is_err());
    let mut value = serde_json::to_value(&canonical).expect("command JSON");
    value
        .as_object_mut()
        .expect("command object")
        .remove("runtime_fence");
    assert!(ProjectDocumentCommand::from_json(&value.to_string()).is_err());

    let mut value = serde_json::to_value(&canonical).expect("command JSON");
    value["request"]["summary"] = Value::Null;
    assert!(ProjectDocumentCommand::from_json(&value.to_string()).is_err());
}

#[test]
fn canonical_text_uuid_revision_and_operation_rules_fail_closed() {
    for title in ["", " title", "title ", "bad\0title"] {
        assert!(active_command(title.to_owned(), None, String::new())
            .validate_for_submission()
            .is_err());
    }
    assert!(
        active_command("title".to_owned(), Some(String::new()), String::new())
            .validate_for_submission()
            .is_err()
    );
    assert!(
        active_command("title".to_owned(), None, "bad\0markdown".to_owned())
            .validate_for_submission()
            .is_err()
    );

    let invalid_id = Uuid::from_u128(1);
    let command = ProjectDocumentCommand::new(
        0,
        DocumentCommandRequest::Create {
            document_id: invalid_id,
            title: "title".to_owned(),
            summary: None,
            content_markdown: String::new(),
        },
    );
    assert!(matches!(
        command.validate_for_submission(),
        Err(DocumentError::InvalidDocumentId { .. })
    ));

    let mut create = active_command("title".to_owned(), None, String::new());
    create.expected_document_revision = 1;
    assert!(create.validate_for_submission().is_err());
    let update_at_zero = ProjectDocumentCommand::new(
        0,
        DocumentCommandRequest::Update {
            document_id: Uuid::parse_str(DOCUMENT).expect("document UUID"),
            title: "title".to_owned(),
            summary: None,
            content_markdown: String::new(),
        },
    );
    assert!(update_at_zero.validate_for_submission().is_err());
    let mut over = active_command("title".to_owned(), None, String::new());
    over.expected_document_revision = MAX_SAFE_REVISION + 1;
    assert!(over.validate_for_submission().is_err());
}

#[test]
fn tombstone_and_metadata_shapes_cannot_smuggle_business_or_source_fields() {
    let head_event: Value = serde_json::from_str(include_str!(
        "../../buzz-sdk/tests/fixtures/project-document-v1/events/head-tombstone.json"
    ))
    .expect("head event");
    let mut head_content: Value =
        serde_json::from_str(head_event["content"].as_str().expect("head content"))
            .expect("head JSON");
    head_content["title"] = json!("leaked old title");
    assert!(serde_json::from_value::<DocumentHeadProjection>(head_content).is_err());

    let meta_event: Value = serde_json::from_str(include_str!(
        "../../buzz-sdk/tests/fixtures/project-document-v1/events/meta-reset.json"
    ))
    .expect("meta event");
    let mut meta: DocumentMetaProjection =
        serde_json::from_str(meta_event["content"].as_str().expect("meta content"))
            .expect("meta JSON");
    meta.source_event_id = Some(
        "1111111111111111111111111111111111111111111111111111111111111111"
            .parse()
            .expect("event ID"),
    );
    assert!(meta.validate().is_err());
}

fn active_command(
    title: String,
    summary: Option<String>,
    content_markdown: String,
) -> ProjectDocumentCommand {
    ProjectDocumentCommand::new(
        0,
        DocumentCommandRequest::Create {
            document_id: Uuid::parse_str(DOCUMENT).expect("document UUID"),
            title,
            summary,
            content_markdown,
        },
    )
}
