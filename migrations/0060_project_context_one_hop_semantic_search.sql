-- Project Context one-hop semantic-search response-only storage fence.
--
-- Kind 40914 is produced only as a signed response to one authenticated
-- generic `/query` request. It is not canonical Nostr state and must never be
-- persisted, indexed, replayed, counted, searched, or fanned out.

ALTER TABLE events
    ADD CONSTRAINT events_kind_not_project_context_one_hop_semantic_search_result
        CHECK (kind <> 40914) NOT VALID;
ALTER TABLE events
    VALIDATE CONSTRAINT events_kind_not_project_context_one_hop_semantic_search_result;
