//! Strict parsing and normalization for schema-v3 Project View mutation receipts.

use nostr::Event;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::relay::SubmitEventResponse;

use super::MutationTarget;

#[derive(Debug, Deserialize)]
pub(super) struct ProjectViewReceipt {
    pub(super) project_revision: u64,
    pub(super) object_id: Option<Uuid>,
    pub(super) object_revision: Option<u64>,
    pub(super) deleted: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectViewObjectReceiptV3 {
    schema_version: u16,
    operation: String,
    project_revision: u64,
    objects: Vec<ProjectViewObjectReceiptEntryV3>,
    #[serde(rename = "continuity_entities")]
    _continuity_entities: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectViewObjectReceiptEntryV3 {
    object_id: Uuid,
    object_type: String,
    object_revision: u64,
    deleted: bool,
}

pub(super) fn parse_receipt(
    response: &SubmitEventResponse,
    event: &Event,
) -> Result<ProjectViewObjectReceiptV3, String> {
    if response.event_id != event.id.to_hex() {
        return Err(
            "Project View integrity error: mutation response event_id differs from the submitted event"
                .to_owned(),
        );
    }
    let payload = response.message.strip_prefix("response:").ok_or_else(|| {
        "Project View integrity error: mutation receipt is missing the canonical `response:` prefix"
            .to_owned()
    })?;
    let value: Value = serde_json::from_str(payload).map_err(|error| {
        format!("Project View integrity error: invalid v3 mutation receipt: {error}")
    })?;
    serde_json::from_value(value).map_err(|error| {
        format!("Project View integrity error: invalid v3 mutation receipt: {error}")
    })
}

pub(super) fn validate_receipt(
    receipt: ProjectViewObjectReceiptV3,
    target: MutationTarget,
) -> Result<ProjectViewReceipt, String> {
    let [object] = receipt.objects.as_slice() else {
        return Err(
            "Project View integrity error: v3 mutation receipt must contain exactly one changed object"
                .to_owned(),
        );
    };
    if receipt.schema_version != 3
        || receipt.operation != target.operation
        || object.object_type != target.object_type.as_str()
        || object.object_id != target.object_id
        || object.object_revision == 0
        || object.deleted != target.deleted
    {
        return Err(
            "Project View integrity error: v3 mutation receipt does not match the requested object"
                .to_owned(),
        );
    }
    Ok(ProjectViewReceipt {
        project_revision: receipt.project_revision,
        object_id: Some(object.object_id),
        object_revision: Some(object.object_revision),
        deleted: Some(object.deleted),
    })
}
