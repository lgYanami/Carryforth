-- Migration 0058.
-- Project Context semantic-query admission foundation. Capability-off and
-- additive: no canonical Project Context data is rewritten.

ALTER TABLE communities
    ADD COLUMN semantic_graph_query_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    ADD CONSTRAINT communities_semantic_graph_query_requires_index
        CHECK (NOT semantic_graph_query_enabled OR semantic_index_enabled);

-- Kind 40912 is a signed, response-only virtual Event. It must never enter the
-- durable Event store through any current or future insertion path.
ALTER TABLE events
    ADD CONSTRAINT events_kind_not_semantic_graph_query_result
        CHECK (kind <> 40912) NOT VALID;
ALTER TABLE events
    VALIDATE CONSTRAINT events_kind_not_semantic_graph_query_result;

-- Cosine distance has no useful direction for a zero vector. NOT VALID keeps
-- rollout additive for historical Foundation rows while rejecting every new
-- zero vector immediately. Query readiness separately rejects a current head
-- that still references historical non-queryable data.
ALTER TABLE semantic_embeddings
    ADD CONSTRAINT semantic_embeddings_nonzero_cosine
        CHECK (vector_norm(embedding) > 0) NOT VALID;

-- Workload lanes are admission limits layered in front of the existing final
-- physical provider gate. Both interactive queries and background indexing use
-- this table; neither workload receives a private physical rate limit.
CREATE TABLE semantic_query_provider_admission (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE NO ACTION,
    provider TEXT NOT NULL,
    workload TEXT NOT NULL
        CHECK (workload IN ('interactive_query', 'background_index')),
    next_admission_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (community_id, provider, workload),
    CHECK (octet_length(btrim(provider)) BETWEEN 1 AND 255)
);

-- Short-lived operator assertion of the complete HTTP routing inventory. The
-- control plane, not this table or one Relay Pod, is responsible for listing
-- every instance currently behind the load balancer. Runtime code strictly
-- revalidates the closed JSON and both digests before use.
CREATE TABLE semantic_graph_http_fleet_attestations (
    community_id UUID NOT NULL REFERENCES communities(id) ON DELETE NO ACTION,
    transport TEXT NOT NULL,
    attestation_id UUID NOT NULL,
    deployment_id TEXT NOT NULL,
    runtime_digest BYTEA NOT NULL,
    inventory_digest BYTEA NOT NULL,
    inventory JSONB NOT NULL,
    routing_inventory_acknowledged_at TIMESTAMPTZ NOT NULL,
    attested_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL,
    attested_by TEXT NOT NULL,
    revoked_at TIMESTAMPTZ,
    revoked_by TEXT,
    PRIMARY KEY (community_id, transport),
    UNIQUE (community_id, attestation_id),
    CONSTRAINT semantic_graph_http_fleet_transport_check
        CHECK (transport = 'http'),
    CONSTRAINT semantic_graph_http_fleet_attestation_id_check
        CHECK (attestation_id <> '00000000-0000-0000-0000-000000000000'::uuid),
    CONSTRAINT semantic_graph_http_fleet_identity_check
        CHECK (
            octet_length(btrim(deployment_id)) BETWEEN 1 AND 128
            AND octet_length(runtime_digest) = 32
            AND octet_length(inventory_digest) = 32
            AND octet_length(btrim(attested_by)) BETWEEN 1 AND 255
        ),
    CONSTRAINT semantic_graph_http_fleet_inventory_shape_check
        CHECK (
            jsonb_typeof(inventory) = 'object'
            AND inventory->>'transport' = transport
            AND inventory->>'deployment_id' = deployment_id
            AND jsonb_typeof(inventory->'instances') = 'array'
            AND jsonb_array_length(inventory->'instances') BETWEEN 1 AND 256
        ),
    CONSTRAINT semantic_graph_http_fleet_lifetime_check
        CHECK (
            routing_inventory_acknowledged_at >= attested_at
            AND routing_inventory_acknowledged_at <= expires_at
            AND expires_at > attested_at
            AND expires_at <= attested_at + INTERVAL '15 minutes'
        ),
    CONSTRAINT semantic_graph_http_fleet_revocation_check
        CHECK (
            (revoked_at IS NULL AND revoked_by IS NULL)
            OR (
                revoked_at IS NOT NULL
                AND revoked_by IS NOT NULL
                AND revoked_at >= attested_at
                AND octet_length(btrim(revoked_by)) BETWEEN 1 AND 255
            )
        )
);
