//! Restart-safe Meeting deadline recovery and durable delivery runtime.

use std::sync::Arc;
use std::time::Duration;

use buzz_core::kind::{
    KIND_MEETING_CREATE, KIND_MEETING_END, KIND_MEETING_GRANT_SIGNAL,
    KIND_MEETING_HUMAN_FLOOR_REQUEST, KIND_MEETING_MODERATOR_COMMAND, KIND_MEETING_OFFER_RESPONSE,
    KIND_MEETING_SPEECH_INTENT, KIND_MEETING_STATE, KIND_STREAM_MESSAGE,
};
use buzz_core::tenant::TenantContext;
use buzz_db::meeting_baton::BatonConfig;
use buzz_db::meeting_floor::{FloorConfig, WinnerSelector};
use chrono::Utc;
use nostr::Event;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::handlers::event::dispatch_persistent_event_now;
use crate::state::AppState;

const DEFAULT_SWEEP_INTERVAL: Duration = Duration::from_millis(100);
const DEFAULT_OUTBOX_LEASE: Duration = Duration::from_secs(30);
const DEFAULT_BATCH_LIMIT: i64 = 100;
const DEFAULT_REVOCATION_LEASE: Duration = Duration::from_secs(30);
const DEFAULT_REVOCATION_SESSION_BATCH: i64 = 32;

/// Load the Meeting V1 timing policy that will be frozen into newly-created
/// Sessions. Existing Sessions always use their persisted copy.
pub(crate) fn baton_config_from_env() -> BatonConfig {
    let defaults = BatonConfig::default();
    let configured = BatonConfig {
        timing_profile_version: defaults.timing_profile_version.clone(),
        agent_offer_ack_ms: env_positive_i64(
            "BUZZ_MEETING_V1_AGENT_OFFER_ACK_MS",
            defaults.agent_offer_ack_ms,
            86_400_000,
        ),
        human_offer_ack_ms: env_positive_i64(
            "BUZZ_MEETING_V1_HUMAN_OFFER_ACK_MS",
            defaults.human_offer_ack_ms,
            86_400_000,
        ),
        moderator_decision_ms: env_positive_i64(
            "BUZZ_MEETING_V1_MODERATOR_DECISION_MS",
            defaults.moderator_decision_ms,
            86_400_000,
        ),
        grant_soft_lease_ms: env_positive_i64(
            "BUZZ_MEETING_V1_GRANT_SOFT_LEASE_MS",
            defaults.grant_soft_lease_ms,
            86_400_000,
        ),
        progress_interval_ms: env_positive_i64(
            "BUZZ_MEETING_V1_PROGRESS_INTERVAL_MS",
            defaults.progress_interval_ms,
            86_400_000,
        ),
        grant_hard_deadline_ms: env_positive_i64(
            "BUZZ_MEETING_V1_GRANT_HARD_DEADLINE_MS",
            defaults.grant_hard_deadline_ms,
            86_400_000,
        ),
        agent_safety_margin_ms: env_positive_i64(
            "BUZZ_MEETING_V1_AGENT_SAFETY_MARGIN_MS",
            defaults.agent_safety_margin_ms,
            86_400_000,
        ),
        max_handoff_depth: env_bounded_i32(
            "BUZZ_MEETING_V1_MAX_HANDOFF_DEPTH",
            defaults.max_handoff_depth,
            0,
            255,
        ),
        max_open_handoffs: env_bounded_i32(
            "BUZZ_MEETING_V1_MAX_OPEN_HANDOFFS",
            defaults.max_open_handoffs,
            1,
            32,
        ),
        moderator_max_rejudgments: env_bounded_i32(
            "BUZZ_MEETING_V1_MODERATOR_MAX_REJUDGMENTS",
            defaults.moderator_max_rejudgments,
            0,
            8,
        ),
        moderator_max_cas_rebases_per_attempt: env_bounded_i32(
            "BUZZ_MEETING_V1_MODERATOR_MAX_CAS_REBASES_PER_ATTEMPT",
            defaults.moderator_max_cas_rebases_per_attempt,
            1,
            64,
        ),
        fallback_policy_version: defaults.fallback_policy_version.clone(),
    };
    if valid_baton_config(&configured) {
        configured
    } else {
        warn!(
            "Ignoring incoherent Meeting V1 timing overrides; using the complete default profile"
        );
        defaults
    }
}

/// Load the Meeting V2 Baton policy independently from the Board budget.
pub(crate) fn v2_baton_config_from_env() -> BatonConfig {
    let defaults = BatonConfig::default();
    let configured = BatonConfig {
        timing_profile_version: buzz_db::meeting_v2::DEFAULT_BATON_TIMING_PROFILE_VERSION
            .to_string(),
        agent_offer_ack_ms: env_positive_i64(
            "BUZZ_MEETING_V2_AGENT_OFFER_ACK_MS",
            defaults.agent_offer_ack_ms,
            86_400_000,
        ),
        human_offer_ack_ms: env_positive_i64(
            "BUZZ_MEETING_V2_HUMAN_OFFER_ACK_MS",
            defaults.human_offer_ack_ms,
            86_400_000,
        ),
        moderator_decision_ms: env_positive_i64(
            "BUZZ_MEETING_V2_MODERATOR_DECISION_MS",
            defaults.moderator_decision_ms,
            86_400_000,
        ),
        grant_soft_lease_ms: env_positive_i64(
            "BUZZ_MEETING_V2_GRANT_SOFT_LEASE_MS",
            defaults.grant_soft_lease_ms,
            86_400_000,
        ),
        progress_interval_ms: env_positive_i64(
            "BUZZ_MEETING_V2_PROGRESS_INTERVAL_MS",
            defaults.progress_interval_ms,
            86_400_000,
        ),
        grant_hard_deadline_ms: env_positive_i64(
            "BUZZ_MEETING_V2_GRANT_HARD_DEADLINE_MS",
            defaults.grant_hard_deadline_ms,
            86_400_000,
        ),
        agent_safety_margin_ms: env_positive_i64(
            "BUZZ_MEETING_V2_AGENT_SAFETY_MARGIN_MS",
            defaults.agent_safety_margin_ms,
            86_400_000,
        ),
        max_handoff_depth: env_bounded_i32(
            "BUZZ_MEETING_V2_MAX_HANDOFF_DEPTH",
            defaults.max_handoff_depth,
            0,
            255,
        ),
        max_open_handoffs: env_bounded_i32(
            "BUZZ_MEETING_V2_MAX_OPEN_HANDOFFS",
            defaults.max_open_handoffs,
            1,
            32,
        ),
        moderator_max_rejudgments: env_bounded_i32(
            "BUZZ_MEETING_V2_MODERATOR_MAX_REJUDGMENTS",
            defaults.moderator_max_rejudgments,
            0,
            8,
        ),
        moderator_max_cas_rebases_per_attempt: env_bounded_i32(
            "BUZZ_MEETING_V2_MODERATOR_MAX_CAS_REBASES_PER_ATTEMPT",
            defaults.moderator_max_cas_rebases_per_attempt,
            1,
            64,
        ),
        fallback_policy_version: defaults.fallback_policy_version.clone(),
    };
    if valid_baton_config(&configured) {
        configured
    } else {
        warn!("Ignoring incoherent Meeting V2 Baton overrides; using the complete default profile");
        BatonConfig {
            timing_profile_version: buzz_db::meeting_v2::DEFAULT_BATON_TIMING_PROFILE_VERSION
                .to_string(),
            ..defaults
        }
    }
}

/// Load the independently frozen Meeting V2 Board Maintenance budget.
pub(crate) fn v2_board_maintenance_ms_from_env() -> i64 {
    env_positive_i64(
        "BUZZ_MEETING_V2_BOARD_MAINTENANCE_MS",
        buzz_db::meeting_v2::DEFAULT_BOARD_MAINTENANCE_MS,
        buzz_db::meeting_baton::MAX_BATON_DURATION_MS,
    )
}

/// Load the configured Claim settle delay, maximum window, and Grant lease.
///
/// Values are milliseconds so integration tests can exercise deadlines without
/// waiting for production defaults. Invalid or out-of-range values fall back to
/// the protocol defaults enforced by `buzz-db`.
pub(crate) fn floor_config_from_env() -> FloorConfig {
    FloorConfig {
        claim_settle_delay: env_duration_ms(
            "BUZZ_MEETING_CLAIM_SETTLE_DELAY_MS",
            buzz_db::meeting_floor::DEFAULT_CLAIM_SETTLE_DELAY,
        ),
        claim_window: env_duration_ms(
            "BUZZ_MEETING_CLAIM_WINDOW_MS",
            buzz_db::meeting_floor::DEFAULT_CLAIM_WINDOW,
        ),
        grant_lease: env_duration_ms(
            "BUZZ_MEETING_GRANT_LEASE_MS",
            buzz_db::meeting_floor::DEFAULT_GRANT_LEASE,
        ),
    }
}

/// Run V0 deadline recovery and the policy-neutral Meeting outbox worker.
///
/// Meeting V1 Create/End/State events share the durable outbox with V0. The
/// V1 creation rollout gate intentionally does not control this worker:
/// disabling new V1 sessions must never strand events for an existing session.
pub async fn run(state: Arc<AppState>) {
    let floor_config = floor_config_from_env();
    let sweep_interval = env_duration_ms("BUZZ_MEETING_SWEEP_INTERVAL_MS", DEFAULT_SWEEP_INTERVAL);
    let outbox_lease = env_duration_ms("BUZZ_MEETING_OUTBOX_LEASE_MS", DEFAULT_OUTBOX_LEASE);
    let revocation_lease =
        env_duration_ms("BUZZ_MEETING_REVOCATION_LEASE_MS", DEFAULT_REVOCATION_LEASE);
    let batch_limit = std::env::var("BUZZ_MEETING_BATCH_LIMIT")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=1000).contains(value))
        .unwrap_or(DEFAULT_BATCH_LIMIT);
    let revocation_session_batch = std::env::var("BUZZ_MEETING_REVOCATION_SESSION_BATCH")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=1000).contains(value))
        .unwrap_or(DEFAULT_REVOCATION_SESSION_BATCH);
    let worker_id = Uuid::new_v4();

    info!(
        claim_settle_delay_ms = floor_config.claim_settle_delay.as_millis(),
        claim_window_ms = floor_config.claim_window.as_millis(),
        grant_lease_ms = floor_config.grant_lease.as_millis(),
        sweep_interval_ms = sweep_interval.as_millis(),
        batch_limit,
        revocation_lease_ms = revocation_lease.as_millis(),
        revocation_session_batch,
        meeting_v1_create_enabled = state.config.meeting_v1_create_enabled,
        meeting_v2_create_enabled = state.config.meeting_v2_create_enabled,
        meeting_v2_direct_actions_create_enabled =
            state.config.meeting_v2_direct_actions_create_enabled,
        "Meeting runtime started"
    );

    let mut ticker = tokio::time::interval(sweep_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;

        if let Err(error) = buzz_db::meeting_floor::recover_due_floors(
            &state.db,
            &state.relay_keypair,
            floor_config,
            WinnerSelector::UniformRandom,
            batch_limit,
        )
        .await
        {
            error!("Meeting V0 floor recovery failed: {error}");
        }

        if let Err(error) = recover_due_batons(&state, batch_limit).await {
            error!("Meeting V1 baton recovery scan failed: {error}");
        }

        if let Err(error) = process_revocation_jobs(
            &state,
            revocation_lease,
            batch_limit,
            revocation_session_batch,
        )
        .await
        {
            error!("Meeting security-revocation worker failed: {error}");
        }

        if let Err(error) =
            dispatch_outbox_batch(&state, worker_id, outbox_lease, batch_limit).await
        {
            error!("Meeting outbox delivery failed: {error}");
        }
    }
}

async fn recover_due_batons(state: &Arc<AppState>, limit: i64) -> Result<(), buzz_db::DbError> {
    let sessions = match buzz_db::meeting_baton::claim_due_baton_sessions(&state.db, limit).await {
        Ok(sessions) => {
            metrics::counter!("meeting_baton_recovery_scan_total", "outcome" => "success")
                .increment(1);
            metrics::histogram!("meeting_baton_recovery_scan_sessions")
                .record(sessions.len() as f64);
            metrics::gauge!("meeting_baton_recovery_scan_last_sessions").set(sessions.len() as f64);
            metrics::gauge!("meeting_baton_recovery_scan_saturated").set(
                if sessions.len() as i64 >= limit {
                    1.0
                } else {
                    0.0
                },
            );
            metrics::counter!("meeting_v1_recovery_scan_total", "outcome" => "success")
                .increment(1);
            metrics::histogram!("meeting_v1_recovery_scan_sessions").record(sessions.len() as f64);
            metrics::gauge!("meeting_v1_recovery_scan_last_sessions").set(sessions.len() as f64);
            metrics::gauge!("meeting_v1_recovery_scan_saturated").set(
                if sessions.len() as i64 >= limit {
                    1.0
                } else {
                    0.0
                },
            );
            sessions
        }
        Err(error) => {
            metrics::counter!("meeting_baton_recovery_scan_total", "outcome" => "error")
                .increment(1);
            metrics::gauge!("meeting_baton_recovery_scan_last_sessions").set(0.0);
            metrics::gauge!("meeting_baton_recovery_scan_saturated").set(0.0);
            metrics::counter!("meeting_v1_recovery_scan_total", "outcome" => "error").increment(1);
            metrics::gauge!("meeting_v1_recovery_scan_last_sessions").set(0.0);
            metrics::gauge!("meeting_v1_recovery_scan_saturated").set(0.0);
            record_worker_error("recovery_scan", None);
            return Err(error);
        }
    };
    for session in sessions {
        let protocol = baton_protocol_label(session.schema_version);
        match buzz_db::meeting_baton::recover_meeting_v1(
            &state.db,
            session.community_id,
            session.session_id,
            &state.relay_keypair,
        )
        .await
        {
            Ok(transitions) => {
                let outcome = recovery_result_outcome(transitions.len());
                let recovery_lag_seconds = elapsed_since(session.next_action_at);
                metrics::counter!(
                    "meeting_baton_recovery_result_total",
                    "protocol" => protocol,
                    "outcome" => outcome
                )
                .increment(1);
                for transition in &transitions {
                    let deadline_type = classify_recovery_deadline(&transition.primary_type);
                    metrics::counter!(
                        "meeting_baton_recovery_transition_total",
                        "protocol" => protocol,
                        "transition" => classify_recovery_transition(&transition.primary_type)
                    )
                    .increment(1);
                    metrics::histogram!(
                        "meeting_baton_recovery_lag_seconds",
                        "protocol" => protocol,
                        "deadline_type" => deadline_type
                    )
                    .record(recovery_lag_seconds);
                    if transition.primary_type == "action_deadline_exceeded" {
                        metrics::counter!(
                            "meeting_v2_action_transition_total",
                            "from" => "action/runnable",
                            "to" => "action/blocked",
                            "reason" => "action_deadline_exceeded"
                        )
                        .increment(1);
                        metrics::counter!(
                            "meeting_v2_action_blocked_total",
                            "reason" => "action_deadline_exceeded"
                        )
                        .increment(1);
                    }
                }
                if protocol == "v1" {
                    metrics::counter!("meeting_v1_recovery_result_total", "outcome" => outcome)
                        .increment(1);
                    for transition in &transitions {
                        let deadline_type = classify_recovery_deadline(&transition.primary_type);
                        metrics::counter!(
                            "meeting_v1_recovery_transition_total",
                            "transition" => classify_recovery_transition(&transition.primary_type)
                        )
                        .increment(1);
                        metrics::histogram!(
                            "meeting_v1_recovery_lag_seconds",
                            "deadline_type" => deadline_type
                        )
                        .record(recovery_lag_seconds);
                    }
                }
                if !transitions.is_empty() {
                    info!(
                        meeting = %session.session_id,
                        transition_count = transitions.len(),
                        "Recovered due Meeting V1 baton transitions"
                    );
                }
            }
            Err(error) => {
                // One corrupt or contended Session must not starve every other
                // due Session in this bounded scan.
                error!(
                    meeting = %session.session_id,
                    community = %session.community_id,
                    "Meeting V1 session recovery failed: {error}"
                );
                metrics::counter!(
                    "meeting_baton_recovery_result_total",
                    "protocol" => protocol,
                    "outcome" => "error"
                )
                .increment(1);
                if protocol == "v1" {
                    metrics::counter!("meeting_v1_recovery_result_total", "outcome" => "error")
                        .increment(1);
                }
                record_worker_error("recovery_session", Some(protocol));
            }
        }
    }
    Ok(())
}

async fn process_revocation_jobs(
    state: &Arc<AppState>,
    lease: Duration,
    job_limit: i64,
    session_limit: i64,
) -> Result<(), buzz_db::DbError> {
    let lease_ms = i64::try_from(lease.as_millis()).map_err(|_| {
        buzz_db::DbError::InvalidData("Meeting revocation lease is too large".to_string())
    })?;
    let jobs =
        buzz_db::meeting_baton::claim_revocation_jobs(&state.db, job_limit, lease_ms).await?;
    for job in jobs {
        if let Err(job_error) = process_revocation_job(state, &job, session_limit).await {
            error!(
                community = %job.community_id,
                revocation_job = %job.job_id,
                claim_attempt = job.claim_token.attempt(),
                "Meeting security-revocation job failed: {job_error}"
            );
            if !buzz_db::meeting_baton::release_revocation_job(
                &state.db,
                job.community_id,
                job.job_id,
                job.claim_token,
                &job_error.to_string(),
            )
            .await?
            {
                warn!(
                    community = %job.community_id,
                    revocation_job = %job.job_id,
                    "Meeting revocation claim was lost before it could be released"
                );
            }
        }
    }
    Ok(())
}

async fn process_revocation_job(
    state: &Arc<AppState>,
    job: &buzz_db::meeting_baton::MeetingRevocationJob,
    session_limit: i64,
) -> Result<(), buzz_db::DbError> {
    let sessions = buzz_db::meeting_revocation::list_revoked_participant_sessions(
        &state.db,
        job.community_id,
        &job.revoked_pubkey,
        job.security_order,
        job.cursor_session_id,
        session_limit,
    )
    .await?;

    let mut last_session_id = None;
    for session in &sessions {
        let outcome = buzz_db::meeting_revocation::end_meeting_for_revocation(
            &state.db,
            job.community_id,
            session.session_id,
            &job.revoked_pubkey,
            &job.revocation_event_id,
            &state.relay_keypair,
        )
        .await?;
        if session.schema_version == buzz_db::meeting_v2::SCHEMA_VERSION {
            metrics::counter!(
                "meeting_v2_end_total",
                "outcome" => "aborted",
                "reason_code" => "participant_revoked",
                "duplicate" => if matches!(
                    outcome,
                    buzz_db::meeting_revocation::RevocationEndOutcome::AlreadyEnded
                ) {
                    "true"
                } else {
                    "false"
                }
            )
            .increment(1);
        }
        last_session_id = Some(session.session_id);
        info!(
            community = %job.community_id,
            meeting = %session.session_id,
            revocation_job = %job.job_id,
            schema_version = session.schema_version,
            floor_policy = %session.floor_policy_version,
            already_ended = matches!(
                outcome,
                buzz_db::meeting_revocation::RevocationEndOutcome::AlreadyEnded
            ),
            "Processed Meeting participant security revocation"
        );
    }

    if revocation_batch_is_complete(sessions.len(), session_limit) {
        if !buzz_db::meeting_baton::complete_revocation_job(
            &state.db,
            job.community_id,
            job.job_id,
            job.claim_token,
        )
        .await?
        {
            warn!(
                community = %job.community_id,
                revocation_job = %job.job_id,
                "Meeting revocation claim was lost before completion"
            );
        }
    } else if let Some(cursor) = last_session_id {
        if !buzz_db::meeting_baton::advance_revocation_job(
            &state.db,
            job.community_id,
            job.job_id,
            job.claim_token,
            cursor,
            Utc::now(),
        )
        .await?
        {
            warn!(
                community = %job.community_id,
                revocation_job = %job.job_id,
                "Meeting revocation claim was lost before cursor advancement"
            );
        }
    } else {
        // A positive session_limit and an incomplete batch imply at least one
        // row. Treat violating that invariant as retryable corruption.
        return Err(buzz_db::DbError::InvalidData(
            "Meeting revocation worker produced an empty incomplete batch".to_string(),
        ));
    }
    Ok(())
}

fn revocation_batch_is_complete(batch_len: usize, session_limit: i64) -> bool {
    i64::try_from(batch_len).is_ok_and(|batch_len| batch_len < session_limit)
}

async fn dispatch_outbox_batch(
    state: &Arc<AppState>,
    worker_id: Uuid,
    lease: Duration,
    limit: i64,
) -> Result<(), buzz_db::DbError> {
    let events = match buzz_db::meeting_floor::claim_outbox_batch(
        &state.db, worker_id, lease, limit,
    )
    .await
    {
        Ok(events) => events,
        Err(error) => {
            record_worker_error("outbox_claim", None);
            return Err(error);
        }
    };
    for item in events {
        let meeting_protocol = meeting_baton_protocol(&item.stored_event.event);
        let delivery_latency_seconds = elapsed_since(item.stored_event.received_at);
        let tenant = TenantContext::resolved(item.community_id, item.host);
        let kind = item.stored_event.event.kind.as_u16() as u32;
        let actor = item.stored_event.event.pubkey.to_hex();
        match dispatch_persistent_event_now(&tenant, state, &item.stored_event, kind, &actor, None)
            .await
        {
            Ok(_) => {
                let delivered = buzz_db::meeting_floor::mark_outbox_delivered(
                    &state.db,
                    item.community_id,
                    item.sequence,
                    worker_id,
                )
                .await;
                match delivered {
                    Ok(true) => {
                        if let Some(protocol) = meeting_protocol {
                            record_baton_outbox_delivery(
                                protocol,
                                "delivered",
                                delivery_latency_seconds,
                            );
                        }
                    }
                    Ok(false) => {
                        if let Some(protocol) = meeting_protocol {
                            record_baton_outbox_delivery(
                                protocol,
                                "claim_lost",
                                delivery_latency_seconds,
                            );
                            record_worker_error("outbox_claim_lost", Some(protocol));
                        }
                        warn!(
                            meeting = %item.session_id,
                            sequence = item.sequence,
                            "Meeting outbox claim was lost before delivery acknowledgement"
                        );
                    }
                    Err(error) => {
                        if let Some(protocol) = meeting_protocol {
                            record_baton_outbox_delivery(
                                protocol,
                                "worker_error",
                                delivery_latency_seconds,
                            );
                            record_worker_error("outbox_ack", Some(protocol));
                        }
                        return Err(error);
                    }
                }
            }
            Err(error) => {
                if let Some(protocol) = meeting_protocol {
                    record_worker_error("outbox_dispatch", Some(protocol));
                }
                let released = buzz_db::meeting_floor::release_outbox(
                    &state.db,
                    item.community_id,
                    item.sequence,
                    worker_id,
                    &error,
                )
                .await;
                match released {
                    Ok(true) => {
                        if let Some(protocol) = meeting_protocol {
                            record_baton_outbox_delivery(
                                protocol,
                                "dispatch_failed",
                                delivery_latency_seconds,
                            );
                        }
                    }
                    Ok(false) => {
                        if let Some(protocol) = meeting_protocol {
                            record_baton_outbox_delivery(
                                protocol,
                                "claim_lost",
                                delivery_latency_seconds,
                            );
                            record_worker_error("outbox_claim_lost", Some(protocol));
                        }
                        warn!(
                            meeting = %item.session_id,
                            sequence = item.sequence,
                            "Meeting outbox claim was lost before failed delivery could be released"
                        );
                    }
                    Err(release_error) => {
                        if let Some(protocol) = meeting_protocol {
                            record_baton_outbox_delivery(
                                protocol,
                                "worker_error",
                                delivery_latency_seconds,
                            );
                            record_worker_error("outbox_release", Some(protocol));
                        }
                        return Err(release_error);
                    }
                }
            }
        }
    }
    Ok(())
}

fn recovery_result_outcome(transition_count: usize) -> &'static str {
    if transition_count == 0 {
        "noop"
    } else {
        "recovered"
    }
}

fn baton_protocol_label(schema_version: i32) -> &'static str {
    match schema_version {
        buzz_db::meeting_baton::SCHEMA_VERSION => "v1",
        buzz_db::meeting_v2::SCHEMA_VERSION => "v2",
        _ => "unknown",
    }
}

fn classify_recovery_transition(primary_type: &str) -> &'static str {
    match primary_type {
        "participant_revoked" => "participant_revoked",
        "offer_timed_out" => "offer_timed_out",
        "grant_soft_expired" => "grant_soft_expired",
        "grant_hard_expired" => "grant_hard_expired",
        "moderator_fallback" => "moderator_fallback",
        "action_deadline_exceeded" => "action_deadline_exceeded",
        _ => "unknown",
    }
}

fn classify_recovery_deadline(primary_type: &str) -> &'static str {
    match primary_type {
        "offer_timed_out" => "offer_ack",
        "grant_soft_expired" => "grant_soft",
        "grant_hard_expired" => "grant_hard",
        "moderator_fallback" => "moderator_decision",
        "participant_revoked" => "security_revocation",
        "action_deadline_exceeded" => "action",
        _ => "unknown",
    }
}

fn elapsed_since(timestamp: chrono::DateTime<Utc>) -> f64 {
    Utc::now()
        .signed_duration_since(timestamp)
        .num_milliseconds()
        .max(0) as f64
        / 1_000.0
}

fn meeting_baton_protocol(event: &Event) -> Option<&'static str> {
    let kind = event.kind.as_u16() as u32;
    if !matches!(
        kind,
        KIND_STREAM_MESSAGE
            | KIND_MEETING_CREATE
            | KIND_MEETING_END
            | KIND_MEETING_STATE
            | KIND_MEETING_SPEECH_INTENT
            | KIND_MEETING_MODERATOR_COMMAND
            | KIND_MEETING_HUMAN_FLOOR_REQUEST
            | KIND_MEETING_OFFER_RESPONSE
            | KIND_MEETING_GRANT_SIGNAL
    ) {
        return None;
    }

    let mut versions = event
        .tags
        .iter()
        .filter(|tag| tag.kind().to_string() == "v");
    let version = versions
        .next()
        .and_then(|tag| tag.content())
        .filter(|_| versions.next().is_none())?;
    match version {
        "2" => Some("v1"),
        "3" => Some("v2"),
        _ => None,
    }
}

fn record_baton_outbox_delivery(
    protocol: &'static str,
    outcome: &'static str,
    latency_seconds: f64,
) {
    metrics::counter!(
        "meeting_baton_outbox_delivery_total",
        "protocol" => protocol,
        "outcome" => outcome
    )
    .increment(1);
    metrics::histogram!(
        "meeting_baton_outbox_delivery_latency_seconds",
        "protocol" => protocol,
        "outcome" => outcome
    )
    .record(latency_seconds);
    if protocol == "v1" {
        metrics::counter!("meeting_v1_outbox_delivery_total", "outcome" => outcome).increment(1);
        metrics::histogram!(
            "meeting_v1_outbox_delivery_latency_seconds",
            "outcome" => outcome
        )
        .record(latency_seconds);
    }
}

fn record_worker_error(worker: &'static str, protocol: Option<&'static str>) {
    metrics::counter!("meeting_baton_worker_errors_total", "worker" => worker).increment(1);
    // Scan/claim failures predate protocol classification and retain the
    // legacy aggregate. Item/session failures only enter the V1 series when
    // the affected object is actually V1.
    if records_legacy_v1_worker_error(protocol) {
        metrics::counter!("meeting_v1_worker_errors_total", "worker" => worker).increment(1);
    }
}

fn records_legacy_v1_worker_error(protocol: Option<&str>) -> bool {
    protocol.is_none() || matches!(protocol, Some("v1"))
}

fn env_duration_ms(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=300_000).contains(value))
        .map(Duration::from_millis)
        .unwrap_or(default)
}

fn env_positive_i64(name: &str, default: i64, maximum: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=maximum).contains(value))
        .unwrap_or(default)
}

fn env_bounded_i32(name: &str, default: i32, minimum: i32, maximum: i32) -> i32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|value| (minimum..=maximum).contains(value))
        .unwrap_or(default)
}

fn valid_baton_config(config: &BatonConfig) -> bool {
    config.progress_interval_ms <= config.grant_soft_lease_ms
        && config.grant_soft_lease_ms <= config.grant_hard_deadline_ms
        && config.agent_safety_margin_ms < config.grant_hard_deadline_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn signed_event(kind: u32, tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::Custom(u16::try_from(kind).expect("test kind")), "")
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .expect("sign test event")
    }

    #[test]
    fn invalid_duration_uses_default() {
        let name = format!("BUZZ_MEETING_TEST_DURATION_{}", Uuid::new_v4());
        assert_eq!(
            env_duration_ms(&name, Duration::from_millis(17)),
            Duration::from_millis(17)
        );
    }

    #[test]
    fn default_baton_config_is_coherent() {
        assert!(valid_baton_config(&BatonConfig::default()));
    }

    #[test]
    fn revocation_cursor_only_completes_on_a_short_batch() {
        assert!(revocation_batch_is_complete(0, 32));
        assert!(revocation_batch_is_complete(31, 32));
        assert!(!revocation_batch_is_complete(32, 32));
    }

    #[test]
    fn recovery_metric_classification_is_closed() {
        assert_eq!(recovery_result_outcome(0), "noop");
        assert_eq!(recovery_result_outcome(1), "recovered");
        assert_eq!(
            classify_recovery_transition("offer_timed_out"),
            "offer_timed_out"
        );
        assert_eq!(
            classify_recovery_transition("grant_hard_expired"),
            "grant_hard_expired"
        );
        assert_eq!(
            classify_recovery_transition("session-or-error-shaped-private-text"),
            "unknown"
        );
        assert_eq!(classify_recovery_deadline("offer_timed_out"), "offer_ack");
        assert_eq!(
            classify_recovery_deadline("grant_soft_expired"),
            "grant_soft"
        );
        assert_eq!(
            classify_recovery_deadline("grant_hard_expired"),
            "grant_hard"
        );
        assert_eq!(
            classify_recovery_deadline("moderator_fallback"),
            "moderator_decision"
        );
        assert_eq!(
            classify_recovery_deadline("session-or-error-shaped-private-text"),
            "unknown"
        );
    }

    #[test]
    fn outbox_protocol_detection_requires_one_supported_version_tag() {
        let v2 = || Tag::parse(["v", "2"]).expect("v2");
        let v3 = || Tag::parse(["v", "3"]).expect("v3");
        let v1 = || Tag::parse(["v", "1"]).expect("v1");

        let state = signed_event(KIND_MEETING_STATE, vec![v2()]);
        assert_eq!(meeting_baton_protocol(&state), Some("v1"));

        let speech = signed_event(KIND_STREAM_MESSAGE, vec![v2()]);
        assert_eq!(meeting_baton_protocol(&speech), Some("v1"));

        let board_state = signed_event(KIND_MEETING_STATE, vec![v3()]);
        assert_eq!(meeting_baton_protocol(&board_state), Some("v2"));

        let missing_version = signed_event(KIND_MEETING_STATE, Vec::new());
        assert_eq!(meeting_baton_protocol(&missing_version), None);

        let old_protocol = signed_event(KIND_MEETING_STATE, vec![v1()]);
        assert_eq!(meeting_baton_protocol(&old_protocol), None);

        let duplicate_version = signed_event(KIND_MEETING_STATE, vec![v2(), v2()]);
        assert_eq!(meeting_baton_protocol(&duplicate_version), None);

        let unrelated = signed_event(42_199, vec![v2()]);
        assert_eq!(meeting_baton_protocol(&unrelated), None);

        assert_eq!(baton_protocol_label(2), "v1");
        assert_eq!(baton_protocol_label(3), "v2");
        assert_eq!(baton_protocol_label(99), "unknown");
        assert!(records_legacy_v1_worker_error(None));
        assert!(records_legacy_v1_worker_error(Some("v1")));
        assert!(!records_legacy_v1_worker_error(Some("v2")));
        assert!(!records_legacy_v1_worker_error(Some("unknown")));
    }
}
