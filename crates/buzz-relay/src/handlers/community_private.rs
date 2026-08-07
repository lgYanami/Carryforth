//! Shared fail-closed helpers for Community-private protocol families.
//!
//! Project View and Project Document share the same credential baseline and
//! current-principal concept. Protocol-specific capability readiness remains
//! separate so either family can fail closed without hiding the other.

use buzz_auth::Scope;
use buzz_core::kind::{
    is_project_document_protocol_kind, KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD,
    KIND_PROJECT_DOCUMENT_META, KIND_PROJECT_DOCUMENT_REVISION,
};
use nostr::Filter;

use crate::state::AppState;

const PROJECT_DOCUMENT_KINDS: [i32; 4] = [
    KIND_PROJECT_DOCUMENT_COMMAND as i32,
    KIND_PROJECT_DOCUMENT_HEAD as i32,
    KIND_PROJECT_DOCUMENT_REVISION as i32,
    KIND_PROJECT_DOCUMENT_META as i32,
];

/// Return whether a filter could match any Project Document protocol kind.
#[must_use]
pub(crate) fn filter_can_match_project_document(filter: &Filter) -> bool {
    filter.kinds.as_ref().is_none_or(|kinds| {
        kinds
            .iter()
            .any(|kind| is_project_document_protocol_kind(kind.as_u16() as u32))
    })
}

/// Return whether a filter explicitly targets only Project Document kinds.
#[must_use]
pub(crate) fn filter_is_exclusively_project_document(filter: &Filter) -> bool {
    filter.kinds.as_ref().is_some_and(|kinds| {
        !kinds.is_empty()
            && kinds
                .iter()
                .all(|kind| is_project_document_protocol_kind(kind.as_u16() as u32))
    })
}

/// Return whether every supplied filter explicitly targets only Documents.
#[must_use]
pub(crate) fn filters_are_exclusively_project_document(filters: &[Filter]) -> bool {
    !filters.is_empty() && filters.iter().all(filter_is_exclusively_project_document)
}

/// Return whether an explicit Document filter asks for unsupported NIP-50
/// search. A kindless search remains governed by the relay's ordinary p-gate
/// and never gains Document visibility.
#[must_use]
pub(crate) fn filter_is_project_document_search(filter: &Filter) -> bool {
    filter.search.is_some()
        && filter.kinds.as_ref().is_some_and(|kinds| {
            kinds
                .iter()
                .any(|kind| is_project_document_protocol_kind(kind.as_u16() as u32))
        })
}

/// Credential-only half of every Community-private protocol read gate.
#[must_use]
pub(crate) fn credential_can_read_community_private(
    scopes: &[Scope],
    channel_ids: Option<&[uuid::Uuid]>,
) -> bool {
    channel_ids.is_none() && (scopes.is_empty() || scopes.contains(&Scope::MessagesRead))
}

/// Closed result of the current Project Document read gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectDocumentReadDecision {
    /// Credential, principal, capability, and projection state are ready.
    Allowed,
    /// The caller is not a current globally-authorized principal.
    Restricted,
    /// The caller is eligible, but the capability must fail closed.
    Unavailable(&'static str),
}

impl ProjectDocumentReadDecision {
    /// Whether stored Document events may be exposed to this caller.
    #[must_use]
    pub(crate) const fn allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// Resolve the complete point/query read gate before any event existence
/// lookup. Timeouts are intentionally absent because they block writes only;
/// active bans and managed-owner eligibility are enforced by the DB helper.
pub(crate) async fn project_document_read_decision(
    state: &AppState,
    community_id: buzz_core::CommunityId,
    pubkey: &[u8],
    scopes: &[Scope],
    channel_ids: Option<&[uuid::Uuid]>,
) -> Result<ProjectDocumentReadDecision, buzz_db::DbError> {
    if !credential_can_read_community_private(scopes, channel_ids) {
        return Ok(ProjectDocumentReadDecision::Restricted);
    }
    if !state
        .db
        .project_document_authorized_pubkey(community_id, pubkey)
        .await?
    {
        return Ok(ProjectDocumentReadDecision::Restricted);
    }
    if state.config.relay_private_key.is_none() {
        return Ok(ProjectDocumentReadDecision::Unavailable("stable_signer"));
    }
    let Some(status) = state.db.project_document_status(community_id).await? else {
        return Ok(ProjectDocumentReadDecision::Unavailable("not_ready"));
    };
    if !status.enabled {
        return Ok(ProjectDocumentReadDecision::Unavailable("disabled"));
    }
    // Migration reads use explicit operator tooling. Community-private Relay
    // reads are an ordinary schema-v3 runtime surface only.
    if status.project_view_schema_version != 3 {
        return Ok(ProjectDocumentReadDecision::Unavailable("schema"));
    }
    if !state
        .db
        .project_document_capability_ready(community_id, &state.relay_keypair.public_key())
        .await?
    {
        return Ok(ProjectDocumentReadDecision::Unavailable("not_ready"));
    }
    Ok(ProjectDocumentReadDecision::Allowed)
}

/// Add all Document protocol kinds to a query's deny set without discarding
/// exclusions installed by another private protocol gate.
pub(crate) fn exclude_project_document_kinds(
    query: &mut buzz_db::EventQuery,
    filter: &Filter,
    document_read_allowed: bool,
) {
    if document_read_allowed || !filter_can_match_project_document(filter) {
        return;
    }
    let excluded = query.excluded_kinds.get_or_insert_with(Vec::new);
    for kind in PROJECT_DOCUMENT_KINDS {
        if !excluded.contains(&kind) {
            excluded.push(kind);
        }
    }
}

/// Result-level guard, including by-ID reads that bypass kind filters.
#[must_use]
pub(crate) fn event_is_visible(kind: u32, document_read_allowed: bool) -> bool {
    !is_project_document_protocol_kind(kind) || document_read_allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{Filter, Kind};

    #[test]
    fn wildcard_mixed_and_by_id_paths_fail_closed() {
        let wildcard = Filter::new();
        assert!(filter_can_match_project_document(&wildcard));
        assert!(!filter_is_exclusively_project_document(&wildcard));

        let document_only = Filter::new().kind(Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16));
        assert!(filter_is_exclusively_project_document(&document_only));
        assert!(filters_are_exclusively_project_document(&[document_only]));

        let mixed = Filter::new().kinds([
            Kind::Custom(KIND_PROJECT_DOCUMENT_REVISION as u16),
            Kind::TextNote,
        ]);
        assert!(filter_can_match_project_document(&mixed));
        assert!(!filter_is_exclusively_project_document(&mixed));

        for kind in PROJECT_DOCUMENT_KINDS {
            assert!(!event_is_visible(kind as u32, false));
            assert!(event_is_visible(kind as u32, true));
        }
        assert!(event_is_visible(1, false));
    }

    #[test]
    fn exclusion_merges_with_existing_private_denies() {
        let filter = Filter::new();
        let mut query = buzz_db::EventQuery::for_community(buzz_core::CommunityId::from_uuid(
            uuid::Uuid::new_v4(),
        ));
        query.excluded_kinds = Some(vec![41001]);
        exclude_project_document_kinds(&mut query, &filter, false);
        let excluded = query.excluded_kinds.expect("excluded kinds");
        assert!(excluded.contains(&41001));
        assert!(PROJECT_DOCUMENT_KINDS
            .iter()
            .all(|kind| excluded.contains(kind)));
    }
}
