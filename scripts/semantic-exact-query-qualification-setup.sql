\set ON_ERROR_STOP on

CREATE EXTENSION IF NOT EXISTS vector;
CREATE SCHEMA semantic_exact_qualification;

CREATE TABLE semantic_exact_qualification.sources (
    community_ordinal INTEGER NOT NULL,
    generation_ordinal INTEGER NOT NULL,
    source_ordinal INTEGER NOT NULL,
    scale_ordinal INTEGER NOT NULL,
    source_family TEXT NOT NULL,
    source_subtype TEXT NOT NULL,
    source_id UUID NOT NULL,
    current_head BOOLEAN NOT NULL,
    authorized BOOLEAN NOT NULL,
    eligible BOOLEAN NOT NULL,
    embedding VECTOR(2048) NOT NULL,
    PRIMARY KEY (community_ordinal, generation_ordinal, source_ordinal)
);

INSERT INTO semantic_exact_qualification.sources (
    community_ordinal,
    generation_ordinal,
    source_ordinal,
    scale_ordinal,
    source_family,
    source_subtype,
    source_id,
    current_head,
    authorized,
    eligible,
    embedding
)
SELECT
    1,
    1,
    source_ordinal,
    source_ordinal,
    CASE source_ordinal % 3
        WHEN 0 THEN 'project_view'
        WHEN 1 THEN 'project_document'
        ELSE 'meeting'
    END,
    CASE source_ordinal % 3
        WHEN 0 THEN 'work'
        WHEN 1 THEN 'document'
        ELSE 'meeting'
    END,
    (
        substr(md5('eligible-' || source_ordinal::text), 1, 8) || '-' ||
        substr(md5('eligible-' || source_ordinal::text), 9, 4) || '-' ||
        substr(md5('eligible-' || source_ordinal::text), 13, 4) || '-' ||
        substr(md5('eligible-' || source_ordinal::text), 17, 4) || '-' ||
        substr(md5('eligible-' || source_ordinal::text), 21, 12)
    )::uuid,
    TRUE,
    TRUE,
    TRUE,
    (
        ARRAY[
            ((source_ordinal % 97) + 1)::real / 97.0::real,
            ((source_ordinal % 89) + 1)::real / 89.0::real,
            ((source_ordinal % 83) + 1)::real / 83.0::real,
            ((source_ordinal % 79) + 1)::real / 79.0::real,
            ((source_ordinal % 73) + 1)::real / 73.0::real,
            ((source_ordinal % 71) + 1)::real / 71.0::real,
            ((source_ordinal % 67) + 1)::real / 67.0::real,
            ((source_ordinal % 61) + 1)::real / 61.0::real
        ] || array_fill(0.0001::real, ARRAY[2040])
    )::vector(2048)
FROM generate_series(1, :target_sources) AS source_ordinal;

-- Distractors deliberately fail Community, generation, current-head,
-- authorization, or eligibility predicates. They prove that the exact
-- distance cross join consumes only the materialized eligible set.
INSERT INTO semantic_exact_qualification.sources (
    community_ordinal,
    generation_ordinal,
    source_ordinal,
    scale_ordinal,
    source_family,
    source_subtype,
    source_id,
    current_head,
    authorized,
    eligible,
    embedding
)
SELECT
    CASE distractor_ordinal % 5 WHEN 0 THEN 2 ELSE 1 END,
    CASE distractor_ordinal % 5 WHEN 1 THEN 2 ELSE 1 END,
    :target_sources + distractor_ordinal,
    1 + ((distractor_ordinal - 1) % :target_sources),
    'project_view',
    'work',
    (
        substr(md5('distractor-' || distractor_ordinal::text), 1, 8) || '-' ||
        substr(md5('distractor-' || distractor_ordinal::text), 9, 4) || '-' ||
        substr(md5('distractor-' || distractor_ordinal::text), 13, 4) || '-' ||
        substr(md5('distractor-' || distractor_ordinal::text), 17, 4) || '-' ||
        substr(md5('distractor-' || distractor_ordinal::text), 21, 12)
    )::uuid,
    distractor_ordinal % 5 <> 2,
    distractor_ordinal % 5 <> 3,
    distractor_ordinal % 5 <> 4,
    (
        ARRAY[
            ((distractor_ordinal % 59) + 1)::real / 59.0::real,
            ((distractor_ordinal % 53) + 1)::real / 53.0::real,
            ((distractor_ordinal % 47) + 1)::real / 47.0::real,
            ((distractor_ordinal % 43) + 1)::real / 43.0::real,
            ((distractor_ordinal % 41) + 1)::real / 41.0::real,
            ((distractor_ordinal % 37) + 1)::real / 37.0::real,
            ((distractor_ordinal % 31) + 1)::real / 31.0::real,
            ((distractor_ordinal % 29) + 1)::real / 29.0::real
        ] || array_fill(0.0001::real, ARRAY[2040])
    )::vector(2048)
FROM generate_series(1, :distractor_sources) AS distractor_ordinal;

CREATE INDEX semantic_exact_qualification_source_gate
    ON semantic_exact_qualification.sources (
        scale_ordinal,
        community_ordinal,
        generation_ordinal,
        current_head,
        authorized,
        eligible,
        source_ordinal
    );

ANALYZE semantic_exact_qualification.sources;

CREATE FUNCTION semantic_exact_qualification.exact_count(
    requested_scale INTEGER,
    requested_channels INTEGER,
    recall_per_channel INTEGER
) RETURNS BIGINT
LANGUAGE SQL
VOLATILE
PARALLEL SAFE
AS $$
WITH pre_gate AS MATERIALIZED (
    SELECT community_ordinal, generation_ordinal, source_ordinal,
           source_family, source_subtype, source_id,
           current_head, authorized, eligible, embedding
    FROM semantic_exact_qualification.sources
    WHERE scale_ordinal <= requested_scale
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
    FROM generate_series(1, requested_channels) AS channel_ordinal
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
WHERE channel_rank <= recall_per_channel
  AND rejected.rejected_count >= 0;
$$;

SELECT json_build_object(
    'eligible_sources', (
        SELECT count(*)
        FROM semantic_exact_qualification.sources
        WHERE community_ordinal = 1
          AND generation_ordinal = 1
          AND current_head
          AND authorized
          AND eligible
    ),
    'distractor_sources', (
        SELECT count(*)
        FROM semantic_exact_qualification.sources
        WHERE NOT (
            community_ordinal = 1
            AND generation_ordinal = 1
            AND current_head
            AND authorized
            AND eligible
        )
    ),
    'vector_dimensions', (
        SELECT vector_dims(embedding)
        FROM semantic_exact_qualification.sources
        LIMIT 1
    )
);
