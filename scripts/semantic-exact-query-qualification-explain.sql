\set ON_ERROR_STOP on

EXPLAIN (ANALYZE, BUFFERS, WAL, SETTINGS, FORMAT JSON)
WITH pre_gate AS MATERIALIZED (
    SELECT community_ordinal, generation_ordinal, source_ordinal,
           source_family, source_subtype, source_id,
           current_head, authorized, eligible, embedding
    FROM semantic_exact_qualification.sources
    WHERE scale_ordinal <= :requested_scale
),
eligible AS MATERIALIZED (
    SELECT source_family, source_subtype, source_id, embedding
    FROM pre_gate
    WHERE community_ordinal = 1
      AND generation_ordinal = 1
      AND current_head
      AND authorized
      AND eligible
),
rejected_by_gate AS MATERIALIZED (
    SELECT source_ordinal
    FROM pre_gate
    WHERE NOT (
        community_ordinal = 1
        AND generation_ordinal = 1
        AND current_head
        AND authorized
        AND eligible
    )
),
query_vectors(channel_id, query_vector) AS MATERIALIZED (
    SELECT
        channel_ordinal,
        (
            ARRAY[
                ((channel_ordinal % 23) + 1)::real / 23.0::real,
                ((channel_ordinal % 19) + 1)::real / 19.0::real,
                ((channel_ordinal % 17) + 1)::real / 17.0::real,
                ((channel_ordinal % 13) + 1)::real / 13.0::real,
                ((channel_ordinal % 11) + 1)::real / 11.0::real,
                ((channel_ordinal % 7) + 1)::real / 7.0::real,
                ((channel_ordinal % 5) + 1)::real / 5.0::real,
                ((channel_ordinal % 3) + 1)::real / 3.0::real
            ] || array_fill(0.0001::real, ARRAY[2040])
        )::vector(2048)
    FROM generate_series(1, :requested_channels) AS channel_ordinal
),
distances AS (
    SELECT
        eligible.source_family,
        eligible.source_subtype,
        eligible.source_id,
        query_vectors.channel_id,
        eligible.embedding <=> query_vectors.query_vector AS distance
    FROM eligible
    CROSS JOIN query_vectors
),
finite_distances AS MATERIALIZED (
    SELECT *
    FROM distances
    WHERE distance > '-Infinity'::double precision
      AND distance < 'Infinity'::double precision
),
ranked AS (
    SELECT
        finite_distances.*,
        floor((
            (greatest(-1.0, least(1.0, 1.0 - distance)) + 1.0) / 2.0
        ) * 1000000.0 + 0.5)::bigint AS semantic_score,
        row_number() OVER (
            PARTITION BY channel_id
            ORDER BY distance, source_family, source_subtype, source_id
        ) AS channel_rank
    FROM finite_distances
)
SELECT count(ranked.*)
FROM ranked
CROSS JOIN (SELECT count(*) AS rejected_count FROM rejected_by_gate) rejected
WHERE channel_rank <= :recall_per_channel
  AND rejected.rejected_count >= 0;
