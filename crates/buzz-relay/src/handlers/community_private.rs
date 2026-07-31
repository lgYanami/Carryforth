//! Shared fail-closed helpers for Community-private protocol families.
//!
//! Project Document stays unadvertised and unreadable throughout Stage 1.
//! These helpers deliberately cover kindless, mixed-kind, search, by-id, and
//! COUNT paths so a mistakenly inserted test/projection event cannot escape
//! through an older wildcard route.

use buzz_core::kind::{
    is_project_document_protocol_kind, KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD,
    KIND_PROJECT_DOCUMENT_META, KIND_PROJECT_DOCUMENT_REVISION,
};
use nostr::Filter;

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

/// Add all Document protocol kinds to a query's deny set without discarding
/// exclusions installed by another private protocol gate.
pub(crate) fn exclude_project_document_kinds(query: &mut buzz_db::EventQuery, filter: &Filter) {
    if !filter_can_match_project_document(filter) {
        return;
    }
    let excluded = query.excluded_kinds.get_or_insert_with(Vec::new);
    for kind in PROJECT_DOCUMENT_KINDS {
        if !excluded.contains(&kind) {
            excluded.push(kind);
        }
    }
}

/// Stage-1 result/fan-out guard, including by-ID reads that bypass kind filters.
#[must_use]
pub(crate) fn event_is_visible(kind: u32) -> bool {
    !is_project_document_protocol_kind(kind)
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
            assert!(!event_is_visible(kind as u32));
        }
        assert!(event_is_visible(1));
    }

    #[test]
    fn exclusion_merges_with_existing_private_denies() {
        let filter = Filter::new();
        let mut query = buzz_db::EventQuery::for_community(buzz_core::CommunityId::from_uuid(
            uuid::Uuid::new_v4(),
        ));
        query.excluded_kinds = Some(vec![41001]);
        exclude_project_document_kinds(&mut query, &filter);
        let excluded = query.excluded_kinds.expect("excluded kinds");
        assert!(excluded.contains(&41001));
        assert!(PROJECT_DOCUMENT_KINDS
            .iter()
            .all(|kind| excluded.contains(kind)));
    }
}
