-- Durable, bounded recovery receipt for the local-owner bootstrap incident.
--
-- The operation never edits Project business rows or revisions. It records an
-- exact operator-authorized restoration of the already-referenced canonical
-- NIP-43 snapshot after a generic publisher incorrectly replaced it with a
-- semantically equal but wire-noncanonical event.

CREATE TABLE project_view_v3_membership_snapshot_recoveries (
    community_id                   UUID        NOT NULL,
    recovery_id                    UUID        NOT NULL,
    idempotency_key_hash           BYTEA       NOT NULL,
    canonical_request_hash         BYTEA       NOT NULL,
    requested_by                   BYTEA       NOT NULL,
    audit_seq                      BIGINT      NOT NULL,
    expected_project_revision      BIGINT      NOT NULL,
    expected_projection_generation BIGINT     NOT NULL,
    restored_membership_event_id   BYTEA       NOT NULL,
    retired_membership_event_id    BYTEA       NOT NULL,
    result_receipt                 JSONB       NOT NULL,
    accepted_at                    TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, recovery_id),
    CONSTRAINT project_view_v3_membership_recovery_idempotency_unique
        UNIQUE (community_id, idempotency_key_hash),
    CONSTRAINT project_view_v3_membership_recovery_community_fk
        FOREIGN KEY (community_id)
        REFERENCES communities (id)
        ON DELETE NO ACTION,
    CONSTRAINT project_view_v3_membership_recovery_audit_fk
        FOREIGN KEY (community_id, audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_v3_membership_recovery_shape_check
        CHECK (
            recovery_id <> '00000000-0000-0000-0000-000000000000'::uuid
            AND octet_length(idempotency_key_hash) = 32
            AND octet_length(canonical_request_hash) = 32
            AND octet_length(requested_by) = 32
            AND audit_seq > 0
            AND expected_project_revision BETWEEN 1 AND 9007199254740991
            AND expected_projection_generation BETWEEN 1 AND 9007199254740991
            AND octet_length(restored_membership_event_id) = 32
            AND octet_length(retired_membership_event_id) = 32
            AND restored_membership_event_id <> retired_membership_event_id
            AND jsonb_typeof(result_receipt) = 'object'
        )
);

CREATE TRIGGER project_view_v3_membership_snapshot_recoveries_immutable
    BEFORE UPDATE OR DELETE ON project_view_v3_membership_snapshot_recoveries
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();
