-- Stage 7: trusted managed-runtime supervision.
--
-- Runtime heartbeats, leases, epochs, and recovery evidence are operational
-- state. They intentionally do not live in Project View projections and do
-- not advance project_revision. Only the final policy-fenced
-- end_unrecoverable_assignment action enters project_view_changes.

CREATE TABLE project_runtime_supervisor_bindings (
    community_id                    UUID        NOT NULL,
    binding_id                      UUID        NOT NULL,
    assignment_id                   UUID        NOT NULL,
    supervisor_pubkey               BYTEA       NOT NULL,
    lease_seconds                   INTEGER     NOT NULL,
    recovery_window_seconds         INTEGER     NOT NULL,
    max_recovery_attempts           INTEGER     NOT NULL,
    recovery_backoff_seconds        INTEGER     NOT NULL,
    monitor_timeout_seconds         INTEGER     NOT NULL,
    monitor_grace_seconds           INTEGER     NOT NULL,
    automatic_unrecoverable         BOOLEAN     NOT NULL DEFAULT FALSE,
    registered_by                   BYTEA       NOT NULL,
    registered_at                   TIMESTAMPTZ NOT NULL,
    revoked_by                      BYTEA,
    revoked_at                      TIMESTAMPTZ,
    last_monitor_at                 TIMESTAMPTZ,
    monitor_grace_until             TIMESTAMPTZ,
    scheduler_claim_token           UUID,
    scheduler_claimed_until         TIMESTAMPTZ,
    system_change_id                BYTEA,
    system_audit_seq                BIGINT,
    updated_at                      TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, binding_id),
    CONSTRAINT project_runtime_bindings_assignment_fk
        FOREIGN KEY (community_id, assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_runtime_bindings_change_fk
        FOREIGN KEY (community_id, system_change_id)
        REFERENCES project_view_changes (community_id, change_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_runtime_bindings_audit_fk
        FOREIGN KEY (community_id, system_audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_runtime_bindings_id_check
        CHECK (binding_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_runtime_bindings_assignment_id_check
        CHECK (assignment_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_runtime_bindings_pubkey_check
        CHECK (
            octet_length(supervisor_pubkey) = 32
            AND octet_length(registered_by) = 32
            AND (revoked_by IS NULL OR octet_length(revoked_by) = 32)
        ),
    CONSTRAINT project_runtime_bindings_policy_check
        CHECK (
            lease_seconds BETWEEN 10 AND 300
            AND recovery_window_seconds BETWEEN 30 AND 86400
            AND max_recovery_attempts BETWEEN 1 AND 100
            AND recovery_backoff_seconds BETWEEN 1 AND 300
            AND recovery_backoff_seconds < recovery_window_seconds
            AND monitor_timeout_seconds BETWEEN 30 AND 3600
            AND monitor_grace_seconds BETWEEN 30 AND 86400
        ),
    CONSTRAINT project_runtime_bindings_revocation_check
        CHECK (
            (revoked_at IS NULL AND revoked_by IS NULL)
            OR (
                revoked_at IS NOT NULL
                AND revoked_by IS NOT NULL
                AND revoked_at >= registered_at
            )
        ),
    CONSTRAINT project_runtime_bindings_monitor_check
        CHECK (
            (last_monitor_at IS NULL AND monitor_grace_until IS NULL)
            OR (
                last_monitor_at IS NOT NULL
                AND monitor_grace_until IS NOT NULL
                AND last_monitor_at >= registered_at
                AND monitor_grace_until >= last_monitor_at
            )
        ),
    CONSTRAINT project_runtime_bindings_claim_check
        CHECK (
            (scheduler_claim_token IS NULL) = (scheduler_claimed_until IS NULL)
            AND (
                scheduler_claim_token IS NULL
                OR scheduler_claim_token <>
                   '00000000-0000-0000-0000-000000000000'::uuid
            )
        ),
    CONSTRAINT project_runtime_bindings_system_change_check
        CHECK (
            (system_change_id IS NULL AND system_audit_seq IS NULL)
            OR (
                octet_length(system_change_id) = 32
                AND system_audit_seq > 0
            )
        ),
    CONSTRAINT project_runtime_bindings_updated_check
        CHECK (updated_at >= registered_at)
);

CREATE UNIQUE INDEX idx_project_runtime_bindings_active_assignment
    ON project_runtime_supervisor_bindings (community_id, assignment_id)
    WHERE revoked_at IS NULL;

CREATE INDEX idx_project_runtime_bindings_supervisor
    ON project_runtime_supervisor_bindings (
        community_id,
        supervisor_pubkey,
        assignment_id
    )
    WHERE revoked_at IS NULL;

CREATE INDEX idx_project_runtime_bindings_scheduler
    ON project_runtime_supervisor_bindings (
        updated_at,
        community_id,
        binding_id
    )
    WHERE revoked_at IS NULL
      AND automatic_unrecoverable
      AND system_change_id IS NULL;

CREATE TABLE project_runtime_leases (
    community_id                    UUID        NOT NULL,
    binding_id                      UUID        NOT NULL,
    assignment_id                   UUID        NOT NULL,
    runtime_id                      UUID        NOT NULL,
    runtime_epoch                   BIGINT      NOT NULL,
    availability                    TEXT        NOT NULL,
    lease_expires_at                TIMESTAMPTZ,
    recovery_started_at             TIMESTAMPTZ,
    recovery_deadline               TIMESTAMPTZ,
    recovery_attempts               INTEGER     NOT NULL DEFAULT 0,
    recovery_attempt_in_flight      BOOLEAN     NOT NULL DEFAULT FALSE,
    next_recovery_at                TIMESTAMPTZ,
    last_evidence_id                BYTEA       NOT NULL,
    last_evidence_at                TIMESTAMPTZ NOT NULL,
    ended_at                        TIMESTAMPTZ,
    created_at                      TIMESTAMPTZ NOT NULL,
    updated_at                      TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, binding_id, runtime_id),
    CONSTRAINT project_runtime_leases_binding_fk
        FOREIGN KEY (community_id, binding_id)
        REFERENCES project_runtime_supervisor_bindings (community_id, binding_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_runtime_leases_assignment_fk
        FOREIGN KEY (community_id, assignment_id)
        REFERENCES project_role_assignments (community_id, assignment_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_runtime_leases_runtime_id_check
        CHECK (runtime_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT project_runtime_leases_epoch_check
        CHECK (runtime_epoch BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_runtime_leases_availability_check
        CHECK (availability IN ('available', 'recovering', 'unavailable')),
    CONSTRAINT project_runtime_leases_evidence_check
        CHECK (octet_length(last_evidence_id) = 32),
    CONSTRAINT project_runtime_leases_recovery_check
        CHECK (
            recovery_attempts BETWEEN 0 AND 100
            AND (
                (
                    availability = 'available'
                    AND lease_expires_at IS NOT NULL
                    AND recovery_started_at IS NULL
                    AND recovery_deadline IS NULL
                    AND recovery_attempts = 0
                    AND NOT recovery_attempt_in_flight
                    AND next_recovery_at IS NULL
                    AND ended_at IS NULL
                )
                OR (
                    availability = 'recovering'
                    AND lease_expires_at IS NULL
                    AND recovery_started_at IS NOT NULL
                    AND recovery_deadline IS NOT NULL
                    AND recovery_deadline >= recovery_started_at
                    AND (
                        (
                            recovery_attempt_in_flight
                            AND next_recovery_at IS NULL
                        )
                        OR (
                            NOT recovery_attempt_in_flight
                            AND next_recovery_at IS NOT NULL
                        )
                    )
                    AND ended_at IS NULL
                )
                OR (
                    availability = 'unavailable'
                    AND lease_expires_at IS NULL
                    AND recovery_started_at IS NOT NULL
                    AND recovery_deadline IS NOT NULL
                    AND recovery_deadline >= recovery_started_at
                    AND recovery_attempts > 0
                    AND NOT recovery_attempt_in_flight
                    AND next_recovery_at IS NULL
                    AND ended_at IS NULL
                )
                OR (
                    ended_at IS NOT NULL
                    AND lease_expires_at IS NULL
                    AND NOT recovery_attempt_in_flight
                    AND next_recovery_at IS NULL
                )
            )
        ),
    CONSTRAINT project_runtime_leases_times_check
        CHECK (
            last_evidence_at >= created_at
            AND updated_at >= created_at
            AND (
                next_recovery_at IS NULL
                OR next_recovery_at >= recovery_started_at
            )
            AND (ended_at IS NULL OR ended_at >= created_at)
        )
);

CREATE INDEX idx_project_runtime_leases_assignment
    ON project_runtime_leases (
        community_id,
        assignment_id,
        availability,
        runtime_id
    )
    WHERE ended_at IS NULL;

CREATE TABLE project_runtime_evidence (
    community_id                    UUID        NOT NULL,
    evidence_id                     BYTEA       NOT NULL,
    idempotency_key_hash            BYTEA       NOT NULL,
    request_hash                    BYTEA       NOT NULL,
    binding_id                      UUID        NOT NULL,
    assignment_id                   UUID        NOT NULL,
    runtime_id                      UUID        NOT NULL,
    runtime_epoch                   BIGINT      NOT NULL,
    supervisor_pubkey               BYTEA       NOT NULL,
    evidence_type                   TEXT        NOT NULL,
    detail                          JSONB       NOT NULL,
    availability_after              TEXT        NOT NULL,
    receipt                         JSONB       NOT NULL,
    recorded_at                     TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, evidence_id),
    CONSTRAINT project_runtime_evidence_binding_fk
        FOREIGN KEY (community_id, binding_id)
        REFERENCES project_runtime_supervisor_bindings (community_id, binding_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_runtime_evidence_lease_fk
        FOREIGN KEY (community_id, binding_id, runtime_id)
        REFERENCES project_runtime_leases (community_id, binding_id, runtime_id)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_runtime_evidence_id_check
        CHECK (
            octet_length(evidence_id) = 32
            AND octet_length(idempotency_key_hash) = 32
            AND octet_length(request_hash) = 32
            AND octet_length(supervisor_pubkey) = 32
        ),
    CONSTRAINT project_runtime_evidence_epoch_check
        CHECK (runtime_epoch BETWEEN 1 AND 9007199254740991),
    CONSTRAINT project_runtime_evidence_type_check
        CHECK (
            evidence_type IN (
                'start',
                'lease_renewed',
                'abnormal_exit',
                'recovery_attempt',
                'recovery_succeeded',
                'recovery_failed',
                'supervisor_heartbeat'
            )
        ),
    CONSTRAINT project_runtime_evidence_availability_check
        CHECK (availability_after IN ('available', 'recovering', 'unavailable')),
    CONSTRAINT project_runtime_evidence_json_check
        CHECK (
            jsonb_typeof(detail) = 'object'
            AND jsonb_typeof(receipt) = 'object'
        )
);

CREATE UNIQUE INDEX idx_project_runtime_evidence_idempotency
    ON project_runtime_evidence (community_id, idempotency_key_hash);

CREATE INDEX idx_project_runtime_evidence_history
    ON project_runtime_evidence (
        community_id,
        assignment_id,
        recorded_at DESC,
        evidence_id DESC
    );

CREATE FUNCTION project_runtime_evidence_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Runtime supervisor evidence is append-only'
        USING ERRCODE = 'check_violation';
END
$$;

CREATE TRIGGER project_runtime_evidence_immutable
    BEFORE UPDATE OR DELETE ON project_runtime_evidence
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_evidence_append_only();

-- Bindings are history facts. Policy, monitor health, scheduler claims, and
-- final system pointers may advance; identity and registration provenance may
-- never be rewritten.
CREATE FUNCTION project_runtime_binding_identity_immutable() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.community_id IS DISTINCT FROM OLD.community_id
       OR NEW.binding_id IS DISTINCT FROM OLD.binding_id
       OR NEW.assignment_id IS DISTINCT FROM OLD.assignment_id
       OR NEW.supervisor_pubkey IS DISTINCT FROM OLD.supervisor_pubkey
       OR NEW.registered_by IS DISTINCT FROM OLD.registered_by
       OR NEW.registered_at IS DISTINCT FROM OLD.registered_at THEN
        RAISE EXCEPTION 'Runtime supervisor binding identity is immutable'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER project_runtime_binding_identity_guard
    BEFORE UPDATE ON project_runtime_supervisor_bindings
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_binding_identity_immutable();

-- Validate the terminal trust chain at transaction commit. In particular, an
-- `unrecoverable` Assignment may only come from the closed system operation
-- linked to one exact binding, immutable evidence, and the Community audit
-- chain. Direct SQL cannot manufacture only part of that graph.
CREATE FUNCTION project_runtime_supervision_validate_community(target_community UUID)
RETURNS VOID
LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM project_runtime_supervisor_bindings binding
        JOIN project_role_assignments assignment
          ON assignment.community_id = binding.community_id
         AND assignment.assignment_id = binding.assignment_id
        LEFT JOIN users agent
          ON agent.community_id = assignment.community_id
         AND agent.pubkey = decode(assignment.member_pubkey, 'hex')
        WHERE binding.community_id = target_community
          AND agent.agent_owner_pubkey IS NULL
    ) THEN
        RAISE EXCEPTION 'Runtime supervision requires an exact managed-Agent Assignment'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_runtime_leases runtime
        JOIN project_runtime_supervisor_bindings binding
          ON binding.community_id = runtime.community_id
         AND binding.binding_id = runtime.binding_id
        WHERE runtime.community_id = target_community
          AND runtime.assignment_id IS DISTINCT FROM binding.assignment_id
    ) THEN
        RAISE EXCEPTION 'Runtime lease does not match its supervisor Assignment'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_runtime_evidence evidence
        JOIN project_runtime_supervisor_bindings binding
          ON binding.community_id = evidence.community_id
         AND binding.binding_id = evidence.binding_id
        JOIN project_runtime_leases runtime
          ON runtime.community_id = evidence.community_id
         AND runtime.binding_id = evidence.binding_id
         AND runtime.runtime_id = evidence.runtime_id
        WHERE evidence.community_id = target_community
          AND (
              evidence.assignment_id IS DISTINCT FROM binding.assignment_id
              OR evidence.assignment_id IS DISTINCT FROM runtime.assignment_id
              OR evidence.supervisor_pubkey IS DISTINCT FROM binding.supervisor_pubkey
          )
    ) THEN
        RAISE EXCEPTION 'Runtime evidence does not match its trusted binding'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_runtime_leases runtime
        LEFT JOIN project_runtime_evidence evidence
          ON evidence.community_id = runtime.community_id
         AND evidence.evidence_id = runtime.last_evidence_id
        WHERE runtime.community_id = target_community
          AND (
              evidence.evidence_id IS NULL
              OR evidence.binding_id IS DISTINCT FROM runtime.binding_id
              OR evidence.assignment_id IS DISTINCT FROM runtime.assignment_id
              OR evidence.runtime_id IS DISTINCT FROM runtime.runtime_id
              OR evidence.runtime_epoch IS DISTINCT FROM runtime.runtime_epoch
              OR evidence.availability_after IS DISTINCT FROM runtime.availability
              OR (evidence.receipt->>'recovery_deadline')::timestamptz
                   IS DISTINCT FROM runtime.recovery_deadline
              OR (evidence.receipt->>'recovery_attempts')::integer
                   IS DISTINCT FROM runtime.recovery_attempts
              OR (evidence.receipt->>'recovery_attempt_in_flight')::boolean
                   IS DISTINCT FROM runtime.recovery_attempt_in_flight
              OR (evidence.receipt->>'next_recovery_at')::timestamptz
                   IS DISTINCT FROM runtime.next_recovery_at
              OR evidence.recorded_at IS DISTINCT FROM runtime.last_evidence_at
          )
    ) THEN
        RAISE EXCEPTION 'Runtime lease is not backed by its exact latest evidence'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_runtime_supervisor_bindings binding
        JOIN project_role_assignments assignment
          ON assignment.community_id = binding.community_id
         AND assignment.assignment_id = binding.assignment_id
        WHERE binding.community_id = target_community
          AND binding.revoked_at IS NULL
          AND binding.system_change_id IS NULL
          AND assignment.ended_at IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'An ended Assignment cannot retain a live runtime binding'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_role_assignments assignment
        LEFT JOIN project_view_changes change
          ON change.community_id = assignment.community_id
         AND change.change_id = assignment.ended_source_change_id
        LEFT JOIN project_runtime_supervisor_bindings binding
          ON binding.community_id = assignment.community_id
         AND binding.assignment_id = assignment.assignment_id
         AND binding.system_change_id = assignment.ended_source_change_id
        LEFT JOIN audit_log audit
          ON audit.community_id = change.community_id
         AND audit.seq = change.source_audit_seq
        LEFT JOIN events projection
          ON projection.community_id = assignment.community_id
         AND projection.id = assignment.projection_event_id
         AND projection.deleted_at IS NULL
        WHERE assignment.community_id = target_community
          AND assignment.ended_reason = 'unrecoverable'
          AND (
              change.change_id IS NULL
              OR change.source_type <> 'system'
              OR change.operation <> 'end_unrecoverable_assignment'
              OR change.actor_pubkey IS NOT NULL
              OR change.acting_assignment_id IS NOT NULL
              OR change.project_revision <> assignment.project_revision
              OR change.subject->>'assignment_id' IS DISTINCT FROM
                   assignment.assignment_id::text
              OR binding.binding_id IS NULL
              OR change.subject->>'binding_id' IS DISTINCT FROM binding.binding_id::text
              OR binding.system_audit_seq IS DISTINCT FROM change.source_audit_seq
              OR binding.revoked_at IS NOT NULL
              OR binding.automatic_unrecoverable
              OR binding.scheduler_claim_token IS NOT NULL
              OR binding.scheduler_claimed_until IS NOT NULL
              OR audit.action IS DISTINCT FROM 'runtime_assignment_unrecoverable'
              OR audit.actor_pubkey IS NOT NULL
              OR projection.id IS NULL
              OR projection.pubkey IS DISTINCT FROM assignment.ended_by
              OR NOT EXISTS (
                  SELECT 1
                  FROM project_runtime_evidence evidence
                  WHERE evidence.community_id = binding.community_id
                    AND evidence.binding_id = binding.binding_id
              )
              OR EXISTS (
                  SELECT 1
                  FROM project_runtime_leases runtime
                  WHERE runtime.community_id = binding.community_id
                    AND runtime.binding_id = binding.binding_id
                    AND runtime.ended_at IS NULL
              )
              OR NOT EXISTS (
                  SELECT 1
                  FROM project_role_handoffs handoff
                  WHERE handoff.community_id = assignment.community_id
                    AND handoff.from_assignment_id = assignment.assignment_id
                    AND handoff.source_change_id = assignment.ended_source_change_id
                    AND handoff.system_generated
                    AND handoff.created_by IS NULL
                    AND handoff.body->>'cause' = 'unrecoverable'
              )
          )
    ) THEN
        RAISE EXCEPTION 'Unrecoverable Assignment is missing its trusted runtime system chain'
            USING ERRCODE = 'check_violation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM project_view_changes change
        WHERE change.community_id = target_community
          AND change.operation = 'end_unrecoverable_assignment'
          AND (
              change.source_type <> 'system'
              OR NOT EXISTS (
                  SELECT 1
                  FROM project_role_assignments assignment
                  JOIN project_runtime_supervisor_bindings binding
                    ON binding.community_id = assignment.community_id
                   AND binding.assignment_id = assignment.assignment_id
                   AND binding.system_change_id = change.change_id
                  WHERE assignment.community_id = change.community_id
                    AND assignment.ended_source_change_id = change.change_id
                    AND assignment.ended_reason = 'unrecoverable'
              )
          )
    ) THEN
        RAISE EXCEPTION 'Runtime system change has no matching terminal Assignment'
            USING ERRCODE = 'check_violation';
    END IF;
END
$$;

CREATE FUNCTION project_runtime_supervision_validate_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM project_runtime_supervision_validate_community(OLD.community_id);
    ELSE
        IF TG_OP = 'UPDATE' AND OLD.community_id IS DISTINCT FROM NEW.community_id THEN
            PERFORM project_runtime_supervision_validate_community(OLD.community_id);
        END IF;
        PERFORM project_runtime_supervision_validate_community(NEW.community_id);
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER project_runtime_bindings_validate
    AFTER INSERT OR UPDATE ON project_runtime_supervisor_bindings
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_supervision_validate_row();

CREATE CONSTRAINT TRIGGER project_runtime_leases_validate
    AFTER INSERT OR UPDATE ON project_runtime_leases
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_supervision_validate_row();

CREATE CONSTRAINT TRIGGER project_runtime_evidence_validate
    AFTER INSERT ON project_runtime_evidence
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_supervision_validate_row();

CREATE CONSTRAINT TRIGGER project_runtime_assignments_validate
    AFTER INSERT OR UPDATE ON project_role_assignments
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_supervision_validate_row();

CREATE CONSTRAINT TRIGGER project_runtime_handoffs_validate
    AFTER INSERT OR UPDATE ON project_role_handoffs
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_supervision_validate_row();

CREATE CONSTRAINT TRIGGER project_runtime_changes_validate
    AFTER INSERT OR UPDATE ON project_view_changes
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION project_runtime_supervision_validate_row();
