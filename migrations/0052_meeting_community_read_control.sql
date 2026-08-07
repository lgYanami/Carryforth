-- Migration 0052.
-- Durable, host-scoped approval state for widening historical Meeting reads
-- from the frozen roster to every current Community principal. This migration
-- is additive: it does not rewrite or delete Meeting, Channel, event, Project,
-- Document, or Project Context data.

ALTER TABLE communities
    ADD COLUMN meeting_community_read_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN meeting_community_read_create_paused BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN legacy_meeting_visibility_watermark BIGINT,
    ADD COLUMN legacy_meeting_visibility_audit_digest BYTEA,
    ADD COLUMN legacy_meeting_visibility_meeting_count BIGINT,
    ADD COLUMN legacy_meeting_visibility_community_source_count BIGINT,
    ADD COLUMN legacy_meeting_visibility_private_source_count BIGINT,
    ADD COLUMN legacy_meeting_visibility_missing_source_count BIGINT,
    ADD COLUMN legacy_meeting_visibility_audited_at TIMESTAMPTZ,
    ADD COLUMN legacy_meeting_visibility_approved_at TIMESTAMPTZ,
    ADD COLUMN legacy_meeting_visibility_approved_by TEXT,
    ADD COLUMN meeting_community_read_enabled_at TIMESTAMPTZ,
    ADD CONSTRAINT chk_legacy_meeting_visibility_watermark
        CHECK (
            legacy_meeting_visibility_watermark IS NULL
            OR legacy_meeting_visibility_watermark >= 0
        ),
    ADD CONSTRAINT chk_legacy_meeting_visibility_audit_digest
        CHECK (
            legacy_meeting_visibility_audit_digest IS NULL
            OR LENGTH(legacy_meeting_visibility_audit_digest) = 32
        ),
    ADD CONSTRAINT chk_legacy_meeting_visibility_counts
        CHECK (
            (legacy_meeting_visibility_meeting_count IS NULL
             AND legacy_meeting_visibility_community_source_count IS NULL
             AND legacy_meeting_visibility_private_source_count IS NULL
             AND legacy_meeting_visibility_missing_source_count IS NULL)
            OR
            (legacy_meeting_visibility_meeting_count >= 0
             AND legacy_meeting_visibility_community_source_count >= 0
             AND legacy_meeting_visibility_private_source_count >= 0
             AND legacy_meeting_visibility_missing_source_count >= 0
             AND legacy_meeting_visibility_meeting_count =
                 legacy_meeting_visibility_community_source_count
                 + legacy_meeting_visibility_private_source_count
                 + legacy_meeting_visibility_missing_source_count)
        ),
    ADD CONSTRAINT chk_legacy_meeting_visibility_audit_shape
        CHECK (
            (legacy_meeting_visibility_watermark IS NULL
             AND legacy_meeting_visibility_audit_digest IS NULL
             AND legacy_meeting_visibility_meeting_count IS NULL
             AND legacy_meeting_visibility_community_source_count IS NULL
             AND legacy_meeting_visibility_private_source_count IS NULL
             AND legacy_meeting_visibility_missing_source_count IS NULL
             AND legacy_meeting_visibility_audited_at IS NULL
             AND legacy_meeting_visibility_approved_at IS NULL
             AND legacy_meeting_visibility_approved_by IS NULL)
            OR
            (legacy_meeting_visibility_watermark IS NOT NULL
             AND legacy_meeting_visibility_audit_digest IS NOT NULL
             AND legacy_meeting_visibility_meeting_count IS NOT NULL
             AND legacy_meeting_visibility_community_source_count IS NOT NULL
             AND legacy_meeting_visibility_private_source_count IS NOT NULL
             AND legacy_meeting_visibility_missing_source_count IS NOT NULL
             AND legacy_meeting_visibility_audited_at IS NOT NULL
             AND (
                 (legacy_meeting_visibility_approved_at IS NULL
                  AND legacy_meeting_visibility_approved_by IS NULL)
                 OR
                 (legacy_meeting_visibility_approved_at IS NOT NULL
                  AND legacy_meeting_visibility_approved_by IS NOT NULL
                  AND OCTET_LENGTH(BTRIM(legacy_meeting_visibility_approved_by))
                      BETWEEN 1 AND 255)
             ))
        ),
    ADD CONSTRAINT chk_meeting_community_read_enable_shape
        CHECK (
            (NOT meeting_community_read_enabled
             AND meeting_community_read_enabled_at IS NULL)
            OR
            (meeting_community_read_enabled
             AND meeting_community_read_enabled_at IS NOT NULL
             AND legacy_meeting_visibility_approved_at IS NOT NULL
             AND legacy_meeting_visibility_approved_by IS NOT NULL)
        );

CREATE FUNCTION meeting_community_read_contract_immutable() RETURNS TRIGGER AS $$
BEGIN
    IF OLD.meeting_community_read_enabled
       AND NOT NEW.meeting_community_read_enabled
    THEN
        RAISE EXCEPTION
            'Meeting Community-read contract cannot be disabled after publication'
            USING ERRCODE = 'check_violation';
    END IF;

    IF OLD.meeting_community_read_enabled
       AND (
           NEW.legacy_meeting_visibility_watermark
               IS DISTINCT FROM OLD.legacy_meeting_visibility_watermark
           OR NEW.legacy_meeting_visibility_audit_digest
               IS DISTINCT FROM OLD.legacy_meeting_visibility_audit_digest
           OR NEW.legacy_meeting_visibility_meeting_count
               IS DISTINCT FROM OLD.legacy_meeting_visibility_meeting_count
           OR NEW.legacy_meeting_visibility_community_source_count
               IS DISTINCT FROM OLD.legacy_meeting_visibility_community_source_count
           OR NEW.legacy_meeting_visibility_private_source_count
               IS DISTINCT FROM OLD.legacy_meeting_visibility_private_source_count
           OR NEW.legacy_meeting_visibility_missing_source_count
               IS DISTINCT FROM OLD.legacy_meeting_visibility_missing_source_count
           OR NEW.legacy_meeting_visibility_audited_at
               IS DISTINCT FROM OLD.legacy_meeting_visibility_audited_at
           OR NEW.legacy_meeting_visibility_approved_at
               IS DISTINCT FROM OLD.legacy_meeting_visibility_approved_at
           OR NEW.legacy_meeting_visibility_approved_by
               IS DISTINCT FROM OLD.legacy_meeting_visibility_approved_by
       )
    THEN
        RAISE EXCEPTION
            'approved legacy Meeting visibility evidence is immutable after publication'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_meeting_community_read_contract_immutable
    BEFORE UPDATE OF
        meeting_community_read_enabled,
        legacy_meeting_visibility_watermark,
        legacy_meeting_visibility_audit_digest,
        legacy_meeting_visibility_meeting_count,
        legacy_meeting_visibility_community_source_count,
        legacy_meeting_visibility_private_source_count,
        legacy_meeting_visibility_missing_source_count,
        legacy_meeting_visibility_audited_at,
        legacy_meeting_visibility_approved_at,
        legacy_meeting_visibility_approved_by
    ON communities
    FOR EACH ROW
    EXECUTE FUNCTION meeting_community_read_contract_immutable();
