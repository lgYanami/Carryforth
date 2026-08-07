//! NIP-45 COUNT handler — aggregate queries with channel access enforcement.

use std::sync::Arc;

use nostr::Filter;
use tracing::warn;

use crate::connection::{AuthState, ConnectionState};
use crate::handlers::req::{
    apply_meeting_read_scope, event_visible_to_reader, filter_can_match_persona_shared_kinds,
    filter_can_match_result_gated_kinds, result_gated_count_safe_for_pushdown,
};
use crate::protocol::RelayMessage;
use crate::state::AppState;

/// Extract a channel UUID from a single filter's `#h` tag.
fn extract_channel_from_filter(filter: &Filter) -> Option<uuid::Uuid> {
    let h_tag = nostr::SingleLetterTag::lowercase(nostr::Alphabet::H);
    filter.generic_tags.get(&h_tag).and_then(|vs| {
        if vs.len() == 1 {
            vs.iter().next()?.parse::<uuid::Uuid>().ok()
        } else {
            None
        }
    })
}

/// Handle a COUNT message: require auth, enforce channel access, execute filters,
/// return aggregate count.
pub async fn handle_count(
    sub_id: String,
    filters: Vec<Filter>,
    conn: Arc<ConnectionState>,
    state: Arc<AppState>,
) {
    // Require auth
    let (pubkey_bytes, token_channel_ids, auth_scopes) = {
        let auth = conn.auth_state.read().await;
        match &*auth {
            AuthState::Authenticated(ctx) => (
                ctx.pubkey.to_bytes().to_vec(),
                ctx.channel_ids.clone(),
                ctx.scopes.clone(),
            ),
            _ => {
                conn.send(RelayMessage::closed(
                    &sub_id,
                    "auth-required: not authenticated",
                ));
                return;
            }
        }
    };

    let project_document_can_match = filters
        .iter()
        .any(super::community_private::filter_can_match_project_document);
    let project_document_exclusive =
        super::community_private::filters_are_exclusively_project_document(&filters);
    let project_document_decision = if project_document_can_match {
        match super::community_private::project_document_read_decision(
            &state,
            conn.tenant.community(),
            &pubkey_bytes,
            &auth_scopes,
            token_channel_ids.as_deref(),
        )
        .await
        {
            Ok(decision) => decision,
            Err(error) => {
                warn!(sub_id = %sub_id, "Project Document COUNT authorization failed: {error}");
                conn.send(RelayMessage::closed(
                    &sub_id,
                    "error:project_document:database",
                ));
                return;
            }
        }
    } else {
        super::community_private::ProjectDocumentReadDecision::Restricted
    };
    if project_document_exclusive {
        match project_document_decision {
            super::community_private::ProjectDocumentReadDecision::Allowed => {}
            super::community_private::ProjectDocumentReadDecision::Restricted => {
                conn.send(RelayMessage::closed(
                    &sub_id,
                    "restricted:project_document:membership_required",
                ));
                return;
            }
            super::community_private::ProjectDocumentReadDecision::Unavailable(reason) => {
                conn.send(RelayMessage::closed(
                    &sub_id,
                    &format!("unavailable:project_document:{reason}"),
                ));
                return;
            }
        }
    }
    let project_document_read_allowed = project_document_decision.allowed();
    if project_document_read_allowed
        && filters
            .iter()
            .any(super::community_private::filter_is_project_document_search)
    {
        conn.send(RelayMessage::closed(
            &sub_id,
            "unsupported:project_document:search",
        ));
        return;
    }

    let project_view_can_match = filters.iter().any(super::project_view::filter_can_match);
    if filters.iter().any(|filter| {
        super::project_view::filter_is_exclusively_project_view(filter)
            && super::project_view::filter_has_unscoped_project_view_projection(filter)
    }) {
        conn.send(RelayMessage::closed(
            &sub_id,
            "unsupported:project_view:v3_projection_filter_required",
        ));
        return;
    }
    let project_view_exclusive = !filters.is_empty()
        && filters
            .iter()
            .all(super::project_view::filter_is_exclusively_project_view);
    let project_view_read_allowed = if project_view_can_match
        && super::project_view::credential_can_read(&auth_scopes, token_channel_ids.as_deref())
    {
        match state
            .db
            .project_view_authorized_pubkey(conn.tenant.community(), &pubkey_bytes)
            .await
        {
            Ok(allowed) => allowed,
            Err(error) => {
                warn!(sub_id = %sub_id, "Project View COUNT authorization failed: {error}");
                conn.send(RelayMessage::closed(&sub_id, "error: database error"));
                return;
            }
        }
    } else {
        false
    };
    let project_view_projection_signer = super::project_view::configured_projection_signer(&state);
    if project_view_exclusive && !project_view_read_allowed {
        conn.send(RelayMessage::closed(
            &sub_id,
            "restricted: Project View requires current Community membership and a global read credential",
        ));
        return;
    }
    if project_view_read_allowed
        && filters
            .iter()
            .any(super::project_view::filter_is_project_view_search)
    {
        conn.send(RelayMessage::closed(
            &sub_id,
            "unsupported:project_view:search",
        ));
        return;
    }

    // P-gated kinds (gift wraps, member notifications, observer frames) require
    // the caller's own pubkey in the #p tag — same enforcement as WS REQ handler.
    let authed_pubkey_hex = hex::encode(&pubkey_bytes);
    if !super::req::p_gated_filters_authorized(&filters, &authed_pubkey_hex) {
        conn.send(RelayMessage::closed(
            &sub_id,
            "restricted: p-gated kinds require #p tag matching your pubkey",
        ));
        return;
    }
    if !super::req::engram_filters_authorized(&filters, &authed_pubkey_hex) {
        conn.send(RelayMessage::closed(
            &sub_id,
            "restricted: agent-engram reads require authors=[self] or #p=[self]",
        ));
        return;
    }
    if !super::req::author_only_filters_authorized(&filters, &authed_pubkey_hex) {
        conn.send(RelayMessage::closed(
            &sub_id,
            "restricted: author-only kinds require authors=[self]",
        ));
        return;
    }

    // Get channels this user can access — same enforcement as WS REQ handler.
    let mut accessible_channels = match state
        .get_accessible_channel_ids_cached(conn.tenant.community(), &pubkey_bytes)
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            warn!(sub_id = %sub_id, "Failed to get accessible channels: {e}");
            conn.send(RelayMessage::closed(&sub_id, "error: database error"));
            return;
        }
    };
    // Narrow to the token's channel scope, mirroring the WS REQ handler. Without
    // this, a scoped token would COUNT events in channels outside its scope via
    // the no-channel-filter SQL pushdown below (which counts every accessible
    // channel). The per-filter targeted-channel repair is bounded by the same
    // scope through `resolve_request_local_access`'s `token_allows` argument.
    if let Some(allowed) = token_channel_ids.as_deref() {
        accessible_channels.retain(|channel_id| allowed.contains(channel_id));
    }

    let meeting_scope = match apply_meeting_read_scope(
        &state,
        conn.tenant.community(),
        &pubkey_bytes,
        &mut accessible_channels,
    )
    .await
    {
        Ok(scope) => scope,
        Err(error) => {
            warn!(sub_id = %sub_id, "Meeting reader security check failed: {error}");
            conn.send(RelayMessage::closed(&sub_id, "error: database error"));
            return;
        }
    };
    if filters.iter().any(|filter| {
        extract_channel_from_filter(filter)
            .is_some_and(|channel_id| meeting_scope.revoked_channels.contains(&channel_id))
    }) {
        conn.send(RelayMessage::closed(
            &sub_id,
            "restricted: meeting access revoked",
        ));
        return;
    }

    // For each filter, count matching events with channel access enforcement.
    let mut total: u64 = 0;
    for filter in &filters {
        // NIP-50 is not a Document read surface, including kindless search.
        let document_visible_for_filter = project_document_read_allowed && filter.search.is_none();
        // Determine if this filter can match author-only kinds — if so, the
        // fast-path count_events() cannot be used because it doesn't do
        // per-event author filtering.
        let needs_author_only_filtering = super::req::filter_can_match_author_only_kinds(filter);
        // Determine if this filter can match kind 30175 (persona) — if so, the
        // fast-path must be bypassed because it has no per-event shared-tag check.
        // A fast count over 30175 would include foreign unshared persona events,
        // leaking the existence of private agent activity.
        let needs_persona_filtering = filter_can_match_persona_shared_kinds(filter);
        // Determine if this filter can match result-gated kinds (44200, 30622)
        // that require a per-event owner check. When the fast SQL path would
        // count matching rows without calling reader_authorized_for_event, a
        // non-owner learns the existence of events they are not allowed to see.
        // The only safe pushdown is when #p is pinned to the authenticated
        // reader's own pubkey.
        let needs_result_gated_filtering = filter_can_match_result_gated_kinds(filter)
            && !result_gated_count_safe_for_pushdown(filter, &authed_pubkey_hex);
        // A closed v3 projection filter still needs strict per-event parsing:
        // retained v2 metadata shares its kind and `t` tag with v3 metadata.
        // Keep this independent of today's generic-tag pushability rules so a
        // future `#t` SQL optimization cannot reintroduce legacy over-counting.
        let needs_v3_projection_filtering =
            super::project_view::filter_requires_v3_projection_post_filter(filter);

        if let Some(ch_id) = extract_channel_from_filter(filter) {
            // Filter targets a specific channel — verify access. Mirrors the WS
            // REQ handler: a cache-negative may be a stale miss on a non-writer
            // pod, so confirm uncached and repair the Vec request-locally via
            // `super::req::resolve_request_local_access` (so a just-added channel
            // is counted, and any later filter on the same channel sees it too).
            let db_is_member = if accessible_channels.contains(&ch_id) {
                None
            } else {
                match state
                    .db
                    .is_member(conn.tenant.community(), ch_id, &pubkey_bytes)
                    .await
                {
                    Ok(member) => Some(member),
                    Err(e) => {
                        warn!(sub_id = %sub_id, "Channel membership confirmation failed: {e}");
                        conn.send(RelayMessage::closed(&sub_id, "error: database error"));
                        return;
                    }
                }
            };
            if !super::req::resolve_request_local_access(
                &mut accessible_channels,
                ch_id,
                token_channel_ids
                    .as_deref()
                    .is_none_or(|allowed| allowed.contains(&ch_id)),
                db_is_member,
            ) {
                continue; // Skip filters targeting inaccessible channels.
            }
            if !meeting_scope.meeting_channels.contains(&ch_id) {
                match buzz_db::meeting::is_meeting_reader_authorized_for_channel(
                    &state.db,
                    conn.tenant.community(),
                    ch_id,
                    &pubkey_bytes,
                )
                .await
                {
                    Ok(Some(false)) => {
                        accessible_channels.retain(|channel_id| *channel_id != ch_id);
                        conn.send(RelayMessage::closed(
                            &sub_id,
                            "restricted: meeting access revoked",
                        ));
                        return;
                    }
                    Ok(Some(true) | None) => {}
                    Err(error) => {
                        warn!(sub_id = %sub_id, "Meeting reader authorization check failed: {error}");
                        conn.send(RelayMessage::closed(&sub_id, "error: database error"));
                        return;
                    }
                }
            }
            // Channel is accessible — count with pushability check.
            let mut query = super::req::build_event_query_from_filter(
                filter,
                &pubkey_bytes,
                &state,
                conn.tenant.community(),
            )
            .await;
            exclude_private_protocols_if_unauthorized(
                &mut query,
                filter,
                project_view_read_allowed,
                document_visible_for_filter,
            );
            // Persona visibility pushdown: pre-filter the fallback query_events
            // candidate page before ORDER/LIMIT.
            if needs_persona_filtering {
                query.persona_reader = Some(pubkey_bytes.clone());
            }
            let author_is_self = filter.authors.as_ref().is_some_and(|authors| {
                !authors.is_empty()
                    && authors
                        .iter()
                        .all(|a| a.to_hex().eq_ignore_ascii_case(&authed_pubkey_hex))
            });
            if super::req::filter_fully_pushable(filter)
                && (!needs_author_only_filtering || author_is_self)
                && !needs_result_gated_filtering
                && !needs_persona_filtering
                && !needs_v3_projection_filtering
            {
                match state.db.count_events(&query).await {
                    Ok(n) => total += n as u64,
                    Err(e) => {
                        conn.send(RelayMessage::closed(&sub_id, &format!("error: {e}")));
                        return;
                    }
                }
            } else {
                // Fallback: query + post-filter for non-pushable constraints.
                let mut q = query;
                super::req::apply_count_fallback_limit(&mut q);
                match state.db.query_events(&q).await {
                    Ok(stored_events) => {
                        if super::req::count_fallback_exceeded(stored_events.len()) {
                            metrics::counter!("buzz_count_fallback_rejections_total").increment(1);
                            conn.send(RelayMessage::closed(
                                &sub_id,
                                "restricted: count filter requires narrower constraints",
                            ));
                            return;
                        }
                        for se in stored_events {
                            if !buzz_core::filter::filters_match(std::slice::from_ref(filter), &se)
                            {
                                continue;
                            }
                            if !event_visible_to_reader(&se.event, &pubkey_bytes) {
                                continue;
                            }
                            if !project_view_read_allowed
                                && buzz_core::kind::is_project_view_protocol_kind(
                                    se.event.kind.as_u16() as u32,
                                )
                            {
                                continue;
                            }
                            if !super::project_view::projection_event_visible_for_filter(
                                filter,
                                &se.event,
                                project_view_projection_signer.as_ref(),
                            ) {
                                continue;
                            }
                            if !super::community_private::event_is_visible(
                                se.event.kind.as_u16() as u32,
                                document_visible_for_filter,
                            ) {
                                continue;
                            }
                            total += 1;
                        }
                    }
                    Err(e) => {
                        conn.send(RelayMessage::closed(&sub_id, &format!("error: {e}")));
                        return;
                    }
                }
            }
        } else {
            // No channel filter — use SQL-level channel_ids pushdown to count
            // only events in accessible channels (+ global events).
            //
            // If the filter has generic tags beyond what SQL can push down
            // (#h, #p single, #d single, #e), we must fall back to
            // query + post-filter to avoid overcounting.
            let mut query = super::req::build_event_query_from_filter(
                filter,
                &pubkey_bytes,
                &state,
                conn.tenant.community(),
            )
            .await;
            exclude_private_protocols_if_unauthorized(
                &mut query,
                filter,
                project_view_read_allowed,
                document_visible_for_filter,
            );
            query.channel_ids = Some(accessible_channels.to_vec());
            // Persona visibility pushdown for the fallback query_events path.
            if needs_persona_filtering {
                query.persona_reader = Some(pubkey_bytes.clone());
            }

            let author_is_self = filter.authors.as_ref().is_some_and(|authors| {
                !authors.is_empty()
                    && authors
                        .iter()
                        .all(|a| a.to_hex().eq_ignore_ascii_case(&authed_pubkey_hex))
            });
            if super::req::filter_fully_pushable(filter)
                && (!needs_author_only_filtering || author_is_self)
                && !needs_result_gated_filtering
                && !needs_persona_filtering
                && !needs_v3_projection_filtering
            {
                query.limit = None; // COUNT doesn't need a row limit
                match state.db.count_events(&query).await {
                    Ok(n) => total += n as u64,
                    Err(e) => {
                        conn.send(RelayMessage::closed(&sub_id, &format!("error: {e}")));
                        return;
                    }
                }
            } else {
                // Fallback: query a bounded candidate set + post-filter.
                super::req::apply_count_fallback_limit(&mut query);
                match state.db.query_events(&query).await {
                    Ok(stored_events) => {
                        if super::req::count_fallback_exceeded(stored_events.len()) {
                            metrics::counter!("buzz_count_fallback_rejections_total").increment(1);
                            conn.send(RelayMessage::closed(
                                &sub_id,
                                "restricted: count filter requires narrower constraints",
                            ));
                            return;
                        }
                        for se in stored_events {
                            if !buzz_core::filter::filters_match(std::slice::from_ref(filter), &se)
                            {
                                continue;
                            }
                            if !event_visible_to_reader(&se.event, &pubkey_bytes) {
                                continue;
                            }
                            if !project_view_read_allowed
                                && buzz_core::kind::is_project_view_protocol_kind(
                                    se.event.kind.as_u16() as u32,
                                )
                            {
                                continue;
                            }
                            if !super::project_view::projection_event_visible_for_filter(
                                filter,
                                &se.event,
                                project_view_projection_signer.as_ref(),
                            ) {
                                continue;
                            }
                            if !super::community_private::event_is_visible(
                                se.event.kind.as_u16() as u32,
                                document_visible_for_filter,
                            ) {
                                continue;
                            }
                            total += 1;
                        }
                    }
                    Err(e) => {
                        conn.send(RelayMessage::closed(&sub_id, &format!("error: {e}")));
                        return;
                    }
                }
            }
        }
    }
    conn.send(RelayMessage::count(&sub_id, total));
}

fn exclude_private_protocols_if_unauthorized(
    query: &mut buzz_db::EventQuery,
    filter: &Filter,
    project_view_read_allowed: bool,
    project_document_read_allowed: bool,
) {
    if !project_view_read_allowed && super::project_view::filter_can_match(filter) {
        query.excluded_kinds = Some(vec![
            buzz_core::kind::KIND_PROJECT_VIEW_MUTATION as i32,
            buzz_core::kind::KIND_PROJECT_VIEW_OBJECT as i32,
            buzz_core::kind::KIND_PROJECT_VIEW_META as i32,
        ]);
    }
    if super::project_view::filter_has_unscoped_project_view_projection(filter) {
        let excluded = query.excluded_kinds.get_or_insert_with(Vec::new);
        for kind in [
            buzz_core::kind::KIND_PROJECT_VIEW_OBJECT as i32,
            buzz_core::kind::KIND_PROJECT_VIEW_META as i32,
        ] {
            if !excluded.contains(&kind) {
                excluded.push(kind);
            }
        }
    }
    super::community_private::exclude_project_document_kinds(
        query,
        filter,
        project_document_read_allowed,
    );
}
