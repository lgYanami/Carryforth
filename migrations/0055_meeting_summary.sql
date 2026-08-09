-- Migration 0055.
-- Source-owned retrieval summary for Meeting metadata-first discovery.

ALTER TABLE meeting_sessions
    ADD COLUMN summary TEXT,
    ADD CONSTRAINT chk_meeting_summary_non_empty
        CHECK (summary IS NULL OR BTRIM(summary) <> '');
