-- Durable, replay-first control receipts for the staged Project Context
-- sub-capability. This migration does not enable any Community.

CREATE TABLE project_view_context_operations (
    community_id           UUID        NOT NULL,
    operation_id           UUID        NOT NULL,
    operation              TEXT        NOT NULL,
    idempotency_key_hash   BYTEA       NOT NULL,
    canonical_request_hash BYTEA       NOT NULL,
    requested_by           BYTEA       NOT NULL,
    closure_protocol_version BIGINT    NOT NULL,
    audit_seq              BIGINT      NOT NULL,
    result_receipt         JSONB       NOT NULL,
    accepted_at            TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (community_id, operation_id),
    CONSTRAINT project_view_context_operations_idempotency_unique
        UNIQUE (community_id, idempotency_key_hash),
    CONSTRAINT project_view_context_operations_community_fk
        FOREIGN KEY (community_id) REFERENCES communities (id) ON DELETE NO ACTION,
    CONSTRAINT project_view_context_operations_audit_fk
        FOREIGN KEY (community_id, audit_seq)
        REFERENCES audit_log (community_id, seq)
        ON DELETE NO ACTION
        DEFERRABLE INITIALLY DEFERRED,
    CONSTRAINT project_view_context_operations_name_check
        CHECK (operation IN ('enable', 'disable')),
    CONSTRAINT project_view_context_operations_shape_check
        CHECK (
            operation_id <> '00000000-0000-0000-0000-000000000000'::uuid
            AND octet_length(idempotency_key_hash) = 32
            AND octet_length(canonical_request_hash) = 32
            AND octet_length(requested_by) = 32
            AND closure_protocol_version BETWEEN 1 AND 9007199254740991
            AND audit_seq > 0
        )
);

CREATE TRIGGER project_view_context_operations_immutable
    BEFORE UPDATE OR DELETE ON project_view_context_operations
    FOR EACH ROW EXECUTE FUNCTION project_view_v3_reject_ledger_mutation();
