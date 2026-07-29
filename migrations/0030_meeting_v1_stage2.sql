-- Meeting V1 stage 2: align deterministic Grant progress vocabulary with the
-- moderated-baton wire contract and keep pending canonical-log delivery cheap.

-- Durable revocation fences compare their real commit-era timestamp with the
-- Session cutoff. `now()` is transaction-start time and can misclassify a
-- Meeting whose Create waited for a revoke/restore transaction.
ALTER TABLE meeting_sessions
    ALTER COLUMN created_at SET DEFAULT clock_timestamp();

-- Session-relative security uses one database-monotonic order rather than
-- wall-clock timestamps: timestamp equality and transaction-start semantics
-- cannot define whether a restore happened before a new Meeting Create.
CREATE SEQUENCE meeting_security_order_seq AS BIGINT START WITH 1;

ALTER TABLE meeting_sessions
    ADD COLUMN security_order BIGINT;

ALTER TABLE meeting_revocation_jobs
    ADD COLUMN security_order BIGINT;

CREATE TEMP TABLE meeting_security_order_backfill (
    object_type TEXT NOT NULL,
    community_id UUID NOT NULL,
    object_id UUID NOT NULL,
    security_order BIGINT NOT NULL
) ON COMMIT DROP;

INSERT INTO meeting_security_order_backfill (
    object_type,
    community_id,
    object_id,
    security_order
)
SELECT object_type,
       community_id,
       object_id,
       row_number() OVER (
           ORDER BY created_at, type_order, community_id, object_id
       )
FROM (
    SELECT 'session'::TEXT AS object_type,
           community_id,
           session_id AS object_id,
           created_at,
           0 AS type_order
    FROM meeting_sessions
    UNION ALL
    SELECT 'revocation'::TEXT,
           community_id,
           job_id,
           created_at,
           1
    FROM meeting_revocation_jobs
) objects;

UPDATE meeting_sessions sessions
SET security_order = backfill.security_order
FROM meeting_security_order_backfill backfill
WHERE backfill.object_type = 'session'
  AND backfill.community_id = sessions.community_id
  AND backfill.object_id = sessions.session_id;

UPDATE meeting_revocation_jobs jobs
SET security_order = backfill.security_order
FROM meeting_security_order_backfill backfill
WHERE backfill.object_type = 'revocation'
  AND backfill.community_id = jobs.community_id
  AND backfill.object_id = jobs.job_id;

SELECT setval(
    'meeting_security_order_seq',
    GREATEST(COALESCE(MAX(security_order), 0), 1),
    COALESCE(MAX(security_order), 0) > 0
)
FROM meeting_security_order_backfill;

ALTER TABLE meeting_sessions
    ALTER COLUMN security_order
        SET DEFAULT nextval('meeting_security_order_seq'),
    ALTER COLUMN security_order SET NOT NULL,
    ADD CONSTRAINT chk_meeting_session_security_order
        CHECK (security_order > 0);

ALTER TABLE meeting_revocation_jobs
    ALTER COLUMN security_order
        SET DEFAULT nextval('meeting_security_order_seq'),
    ALTER COLUMN security_order SET NOT NULL,
    ADD CONSTRAINT chk_meeting_revocation_security_order
        CHECK (security_order > 0);

ALTER TABLE meeting_grant_progress
    DROP CONSTRAINT meeting_grant_progress_stage_check,
    ADD CONSTRAINT chk_meeting_progress_stage
        CHECK (stage IN (
            'context_sync',
            'tool_use',
            'generating',
            'composing',
            'submitting'
        ));

CREATE INDEX idx_meeting_event_outbox_pending_session_sequence
    ON meeting_event_outbox (community_id, session_id, sequence)
    WHERE delivered_at IS NULL;

-- A failed/corrupt due Session must not permanently occupy a bounded worker's
-- first LIMIT slots. Claiming a hint advances this retry fence atomically;
-- successful transitions reset it in the state-machine write.
ALTER TABLE meeting_baton_state
    ADD COLUMN recovery_retry_at TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    ADD COLUMN recovery_attempts INT NOT NULL DEFAULT 0,
    ADD CONSTRAINT chk_meeting_baton_recovery_attempts
        CHECK (recovery_attempts >= 0);

CREATE INDEX idx_meeting_baton_state_recovery_due
    ON meeting_baton_state (
        next_action_at,
        recovery_retry_at,
        community_id,
        session_id
    )
    WHERE next_action_at IS NOT NULL;

-- REQ/COUNT and live fan-out enforce permanent, per-Session read revocation
-- even after a principal is re-added or unbanned. Keep that fence an indexed
-- lookup rather than scanning the durable job history for every recipient.
CREATE INDEX idx_meeting_revocation_jobs_reader_fence
    ON meeting_revocation_jobs (community_id, revoked_pubkey, security_order);
