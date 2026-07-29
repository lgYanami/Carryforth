-- Supervisor Adapter: distinguish deliberate runtime retirement from failure.
--
-- A graceful stop closes only one runtime lease. It does not start a recovery
-- episode, contribute unavailable evidence, or end the Assignment.

ALTER TABLE project_runtime_evidence
    DROP CONSTRAINT project_runtime_evidence_type_check;

ALTER TABLE project_runtime_evidence
    ADD CONSTRAINT project_runtime_evidence_type_check
    CHECK (
        evidence_type IN (
            'start',
            'lease_renewed',
            'graceful_stop',
            'abnormal_exit',
            'recovery_attempt',
            'recovery_succeeded',
            'recovery_failed',
            'supervisor_heartbeat'
        )
    );
