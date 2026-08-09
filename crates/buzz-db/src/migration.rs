//! Embedded SQLx migrations for Buzz.
//!
//! Fresh deployments apply the checked-in SQL files under `migrations/`. The
//! multi-tenant rewrite owns a clean consolidated `0001`; legacy single-tenant
//! cutover/backfill is a separate operator script, not startup migration state.

use sqlx::PgPool;

use crate::Result;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Run all pending Buzz database migrations.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    reject_legacy_nip_rs_cardinality_ambiguity(pool).await?;
    MIGRATOR.run(pool).await?;
    // The replica-fence proof (see `replica_fence`) requires the commit-time
    // `created_at` floor trigger from migration 0021 — correctly shaped — on
    // the `events` parent and every partition. `CREATE TABLE .. PARTITION OF`
    // clones parent triggers, but a partition attached with `ATTACH
    // PARTITION` or created by an older code path would silently escape the
    // guard, so migration fails closed if any is missing. (The fence probe
    // re-runs this same check at startup on non-migrating relays.)
    crate::replica_fence::verify_floor_guard_catalog(pool).await?;
    Ok(())
}

/// Migration 0007 is checksum-frozen and predates exact NIP-RS tag-cardinality
/// enforcement. A populated database still on 0001-0006 must not let 0007
/// irreversibly purge duplicate-tag history. Fail before sqlx starts its
/// migration transaction so an operator can inspect and repair those rows.
async fn reject_legacy_nip_rs_cardinality_ambiguity(pool: &PgPool) -> Result<()> {
    let migrations_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations')::text")
            .fetch_one(pool)
            .await?;
    if migrations_table.is_none() {
        return Ok(());
    }
    let applied: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(pool)
            .await?;
    if applied.is_none_or(|version| version >= 7) {
        return Ok(());
    }

    let ambiguous: bool = sqlx::query_scalar(
        "SELECT EXISTS (\
             SELECT 1 FROM events e \
             WHERE e.kind = 30078 \
               AND e.d_tag ~ '^read-state:[0-9a-f]{32}$' \
               AND (\
                   jsonb_typeof(e.tags) IS DISTINCT FROM 'array' \
                   OR (\
                       EXISTS (\
                           SELECT 1 FROM jsonb_array_elements(\
                               CASE WHEN jsonb_typeof(e.tags) = 'array' THEN e.tags ELSE '[]'::jsonb END\
                           ) tag \
                           WHERE tag = '[\"t\", \"read-state\"]'::jsonb\
                       ) \
                       AND (\
                           (SELECT count(*) FROM jsonb_array_elements(\
                               CASE WHEN jsonb_typeof(e.tags) = 'array' THEN e.tags ELSE '[]'::jsonb END\
                            ) tag \
                            WHERE jsonb_typeof(tag) = 'array' \
                              AND tag->0 = '\"d\"'::jsonb) <> 1 \
                           OR NOT EXISTS (\
                               SELECT 1 FROM jsonb_array_elements(\
                                   CASE WHEN jsonb_typeof(e.tags) = 'array' THEN e.tags ELSE '[]'::jsonb END\
                               ) tag \
                               WHERE jsonb_typeof(tag) = 'array' \
                                 AND jsonb_array_length(tag) >= 2 \
                                 AND jsonb_typeof(tag->1) = 'string' \
                                 AND tag->>0 = 'd' \
                                 AND tag->>1 = e.d_tag\
                           ) \
                           OR (SELECT count(*) FROM jsonb_array_elements(\
                               CASE WHEN jsonb_typeof(e.tags) = 'array' THEN e.tags ELSE '[]'::jsonb END\
                           ) tag WHERE tag = '[\"t\", \"read-state\"]'::jsonb) <> 1\
                       )\
                   )\
               )\
         )",
    )
    .fetch_one(pool)
    .await?;

    if ambiguous {
        return Err(crate::DbError::InvalidData(
            "NIP-RS migration blocked: pre-0007 database contains kind-30078 rows with ambiguous d/t tag cardinality; repair or remove those nonconforming rows before retrying"
                .into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ConstraintKind {
        ForeignKey,
        PrimaryKey,
        Unique,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ConstraintLint {
        table: String,
        kind: ConstraintKind,
        description: String,
        columns: Vec<String>,
    }

    /// Concatenated SQL of every embedded migration, in version order.
    ///
    /// The tenant-isolation lints must cover objects introduced by *any*
    /// migration, not just the consolidated `0001`. Concatenating keeps that
    /// coverage honest as additive migrations (e.g. `0002_git_repo_names`) land.
    fn migration_sql() -> String {
        let mut migrations: Vec<_> = MIGRATOR.iter().collect();
        migrations.sort_by_key(|migration| migration.version);
        assert!(
            !migrations.is_empty(),
            "at least the initial migration must exist"
        );
        migrations
            .iter()
            .map(|migration| migration.sql.as_ref())
            .collect::<Vec<&str>>()
            .join("\n")
    }

    fn strip_sql_comments(sql: &str) -> String {
        sql.lines()
            .map(|line| line.split_once("--").map_or(line, |(before, _)| before))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn normalize_sql(sql: &str) -> String {
        strip_sql_comments(sql)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    }

    fn split_sql_statements(sql: &str) -> Vec<String> {
        let sql = strip_sql_comments(sql);
        let bytes = sql.as_bytes();
        let mut statements = Vec::new();
        let mut start = 0usize;
        let mut idx = 0usize;
        let mut in_single_quote = false;
        let mut in_dollar_quote = false;

        while idx < bytes.len() {
            match bytes[idx] {
                b'\'' if !in_dollar_quote => {
                    in_single_quote = !in_single_quote;
                    idx += 1;
                }
                b'$' if !in_single_quote && idx + 1 < bytes.len() && bytes[idx + 1] == b'$' => {
                    in_dollar_quote = !in_dollar_quote;
                    idx += 2;
                }
                b';' if !in_single_quote && !in_dollar_quote => {
                    let statement = sql[start..idx].trim();
                    if !statement.is_empty() {
                        statements.push(statement.to_owned());
                    }
                    start = idx + 1;
                    idx += 1;
                }
                _ => idx += 1,
            }
        }

        let tail = sql[start..].trim();
        if !tail.is_empty() {
            statements.push(tail.to_owned());
        }

        statements
    }

    fn find_matching_paren(sql: &str, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (offset, byte) in sql.as_bytes()[open..].iter().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(open + offset);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn split_top_level_csv(input: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut start = 0usize;
        let mut depth = 0usize;
        for (idx, byte) in input.bytes().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    parts.push(input[start..idx].trim().to_owned());
                    start = idx + 1;
                }
                _ => {}
            }
        }
        let tail = input[start..].trim();
        if !tail.is_empty() {
            parts.push(tail.to_owned());
        }
        parts
    }

    fn identifier_after_keyword(statement: &str, keyword: &str) -> Option<String> {
        let lower = statement.to_ascii_lowercase();
        let keyword_pos = lower.find(keyword)?;
        let mut remainder = statement[keyword_pos + keyword.len()..].trim_start();
        for prefix in ["if not exists", "if exists", "only"] {
            if remainder.to_ascii_lowercase().starts_with(prefix) {
                remainder = remainder[prefix.len()..].trim_start();
            }
        }

        let identifier = remainder
            .split(|ch: char| ch.is_whitespace() || ch == '(')
            .next()?
            .trim_matches('"')
            .rsplit('.')
            .next()?
            .trim_matches('"')
            .to_ascii_lowercase();
        (!identifier.is_empty()).then_some(identifier)
    }

    fn first_parenthesized_columns(input: &str) -> Vec<String> {
        let Some(open) = input.find('(') else {
            return Vec::new();
        };
        let Some(close) = find_matching_paren(input, open) else {
            return Vec::new();
        };

        split_top_level_csv(&input[open + 1..close])
            .into_iter()
            .filter_map(|column| {
                let name = column
                    .trim()
                    .trim_matches('"')
                    .split_whitespace()
                    .next()?
                    .trim_matches('"')
                    .to_ascii_lowercase();
                (!name.is_empty()).then_some(name)
            })
            .collect()
    }

    fn column_definition_name(definition: &str) -> Option<String> {
        let trimmed = definition.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("constraint ")
            || lower.starts_with("primary key")
            || lower.starts_with("foreign key")
            || lower.starts_with("unique")
            || lower.starts_with("check ")
            || lower.starts_with("exclude ")
        {
            return None;
        }

        let name = trimmed
            .split_whitespace()
            .next()?
            .trim_matches('"')
            .to_ascii_lowercase();
        (!name.is_empty()).then_some(name)
    }

    fn create_table_body(statement: &str) -> Option<(String, Vec<String>)> {
        let table = identifier_after_keyword(statement, "create table")?;
        let open = statement.find('(')?;
        let close = find_matching_paren(statement, open)?;
        Some((table, split_top_level_csv(&statement[open + 1..close])))
    }

    fn create_table_definitions(sql: &str) -> Vec<(String, Vec<String>)> {
        split_sql_statements(sql)
            .into_iter()
            .filter_map(|statement| {
                let normalized = statement.trim_start().to_ascii_lowercase();
                if !normalized.starts_with("create table") || normalized.contains(" partition of ")
                {
                    return None;
                }
                create_table_body(&statement)
            })
            .collect()
    }

    fn create_tables(sql: &str) -> BTreeSet<String> {
        create_table_definitions(sql)
            .into_iter()
            .map(|(table, _)| table)
            .collect()
    }

    fn table_has_not_null_community_id(definitions: &[String]) -> bool {
        definitions.iter().any(|definition| {
            column_definition_name(definition).as_deref() == Some("community_id")
                && normalize_sql(definition).contains("not null")
        })
    }

    fn operator_global_tables(sql: &str) -> BTreeSet<String> {
        let mut globals = BTreeSet::new();
        let normalized = normalize_sql(sql);
        let Some(insert_pos) = normalized.find("insert into _operator_global_tables") else {
            return globals;
        };

        for value in [
            "communities",
            "rate_limit_violations",
            "_operator_global_tables",
            "push_gateway_challenges",
            "push_gateway_installations",
            "push_gateway_delegations",
            "push_gateway_endpoint_quotas",
            "push_gateway_delivery_auth_replays",
            "push_gateway_delivery_request_replays",
            "product_feedback",
        ] {
            if normalized[insert_pos..].contains(&format!("'{value}'")) {
                globals.insert(value.to_owned());
            }
        }

        globals
    }

    fn scoped_tables(sql: &str) -> BTreeSet<String> {
        let globals = operator_global_tables(sql);
        create_tables(sql)
            .into_iter()
            .filter(|table| !globals.contains(table))
            .collect()
    }

    fn constraint_lint_for_definition(table: &str, definition: &str) -> Option<ConstraintLint> {
        let normalized = normalize_sql(definition);
        let definition_without_name = if normalized.starts_with("constraint ") {
            let after_constraint = definition
                .trim_start()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or("");
            normalize_sql(after_constraint)
        } else {
            normalized.clone()
        };

        if definition_without_name.starts_with("primary key") {
            Some(ConstraintLint {
                table: table.to_owned(),
                kind: ConstraintKind::PrimaryKey,
                description: definition.to_owned(),
                columns: first_parenthesized_columns(&definition_without_name),
            })
        } else if definition_without_name.starts_with("unique") {
            Some(ConstraintLint {
                table: table.to_owned(),
                kind: ConstraintKind::Unique,
                description: definition.to_owned(),
                columns: first_parenthesized_columns(&definition_without_name),
            })
        } else if definition_without_name.starts_with("foreign key") {
            Some(ConstraintLint {
                table: table.to_owned(),
                kind: ConstraintKind::ForeignKey,
                description: definition.to_owned(),
                columns: first_parenthesized_columns(&definition_without_name),
            })
        } else if normalized.contains(" primary key") {
            column_definition_name(definition).map(|column| ConstraintLint {
                table: table.to_owned(),
                kind: ConstraintKind::PrimaryKey,
                description: definition.to_owned(),
                columns: vec![column],
            })
        } else if normalized.contains(" references ") {
            column_definition_name(definition).map(|column| ConstraintLint {
                table: table.to_owned(),
                kind: ConstraintKind::ForeignKey,
                description: definition.to_owned(),
                columns: vec![column],
            })
        } else if normalized.contains(" unique") {
            column_definition_name(definition).map(|column| ConstraintLint {
                table: table.to_owned(),
                kind: ConstraintKind::Unique,
                description: definition.to_owned(),
                columns: vec![column],
            })
        } else {
            None
        }
    }

    fn table_constraints(sql: &str, scoped_tables: &BTreeSet<String>) -> Vec<ConstraintLint> {
        create_table_definitions(sql)
            .into_iter()
            .filter(|(table, _)| scoped_tables.contains(table))
            .flat_map(|(table, definitions)| {
                definitions.into_iter().filter_map(move |definition| {
                    constraint_lint_for_definition(&table, &definition)
                })
            })
            .collect()
    }

    fn alter_table_constraints(sql: &str, scoped_tables: &BTreeSet<String>) -> Vec<ConstraintLint> {
        split_sql_statements(sql)
            .into_iter()
            .filter_map(|statement| {
                let normalized = normalize_sql(&statement);
                if !normalized.starts_with("alter table") {
                    return None;
                }

                let table = identifier_after_keyword(&statement, "alter table")?;
                if !scoped_tables.contains(&table) {
                    return None;
                }

                let add_pos = normalized.find(" add ")?;
                let definition = normalized[add_pos + " add ".len()..].trim();
                constraint_lint_for_definition(&table, definition)
            })
            .collect()
    }

    fn unique_indexes(sql: &str, scoped_tables: &BTreeSet<String>) -> Vec<ConstraintLint> {
        split_sql_statements(sql)
            .into_iter()
            .filter_map(|statement| {
                let normalized = normalize_sql(&statement);
                if !normalized.starts_with("create unique index") {
                    return None;
                }

                let lower_statement = statement.to_ascii_lowercase();
                let on_pos = lower_statement.find(" on ")?;
                let table = statement[on_pos + " on ".len()..]
                    .trim_start()
                    .split(|ch: char| ch.is_whitespace() || ch == '(')
                    .next()?
                    .trim_matches('"')
                    .rsplit('.')
                    .next()?
                    .trim_matches('"')
                    .to_ascii_lowercase();

                scoped_tables.contains(&table).then(|| ConstraintLint {
                    table,
                    kind: ConstraintKind::Unique,
                    description: statement.clone(),
                    columns: first_parenthesized_columns(&statement[on_pos + " on ".len()..]),
                })
            })
            .collect()
    }

    fn scoped_constraint_lints(sql: &str, scoped_tables: &BTreeSet<String>) -> Vec<ConstraintLint> {
        let mut constraints = table_constraints(sql, scoped_tables);
        constraints.extend(alter_table_constraints(sql, scoped_tables));
        constraints.extend(unique_indexes(sql, scoped_tables));
        constraints
    }

    fn is_allowed_partition_primary_key_exception(constraint: &ConstraintLint) -> bool {
        constraint.table == "delivery_log"
            && constraint.kind == ConstraintKind::PrimaryKey
            && constraint.columns == ["delivered_at", "id"]
    }

    fn scoped_constraint_violations(sql: &str) -> Vec<ConstraintLint> {
        let scoped_tables = scoped_tables(sql);
        scoped_constraint_lints(sql, &scoped_tables)
            .into_iter()
            .filter(|constraint| {
                if is_allowed_partition_primary_key_exception(constraint) {
                    return false;
                }
                constraint.columns.first().map(String::as_str) != Some("community_id")
            })
            .collect()
    }

    fn has_channels_community_id_immutability_guard(sql: &str) -> bool {
        let normalized = normalize_sql(sql);
        normalized.contains("create trigger")
            && normalized.contains("before update")
            && normalized.contains(" on channels")
            && normalized.contains("community_id")
            && normalized.contains("old.community_id")
            && normalized.contains("new.community_id")
            && normalized.contains("raise exception")
    }

    fn forbidden_channels_community_id_mutations(sql: &str) -> Vec<String> {
        split_sql_statements(sql)
            .into_iter()
            .filter(|statement| {
                let normalized = normalize_sql(statement);
                let updates_channels =
                    identifier_after_keyword(statement, "update").as_deref() == Some("channels");
                let update_assignments = normalized
                    .split_once(" set ")
                    .map(|(_, tail)| tail.split_once(" where ").map_or(tail, |(set, _)| set));
                let mutates_with_update = updates_channels
                    && update_assignments
                        .is_some_and(|assignments| assignments.contains("community_id"));
                let alters_channels = identifier_after_keyword(statement, "alter table").as_deref()
                    == Some("channels");
                let drops_channels = identifier_after_keyword(statement, "drop table").as_deref()
                    == Some("channels");
                let drops_or_rewrites_column = alters_channels
                    && (normalized.contains("drop column community_id")
                        || normalized.contains("alter column community_id")
                        || normalized.contains("rename column community_id")
                        || normalized.contains("rename community_id")
                        || normalized.contains("drop trigger")
                        || normalized.contains("disable trigger"));

                mutates_with_update || drops_or_rewrites_column || drops_channels
            })
            .collect()
    }

    #[test]
    fn embedded_migrator_contains_consolidated_initial_schema() {
        let mut migrations: Vec<_> = MIGRATOR.iter().collect();
        migrations.sort_by_key(|migration| migration.version);

        assert_eq!(migrations.len(), 55);
        assert_eq!(migrations[0].version, 1);
        assert_eq!(&*migrations[0].description, "initial schema");
        assert!(migrations[0]
            .sql
            .as_str()
            .contains("CREATE TABLE communities"));
        assert!(migrations[0].sql.as_str().contains("CREATE TABLE channels"));
        assert!(migrations[0]
            .sql
            .as_str()
            .contains("CREATE TABLE scheduled_workflow_fires"));
        assert!(migrations[0]
            .sql
            .as_str()
            .contains("CREATE TABLE audit_log"));
        assert!(migrations[0]
            .sql
            .as_str()
            .contains("CREATE TABLE _operator_global_tables"));
        assert!(migrations[0]
            .sql
            .as_str()
            .contains("search_tsv  TSVECTOR GENERATED ALWAYS"));

        // The git repo-name registry is an additive migration, never folded into
        // 0001 — folding it would change 0001's checksum and break brownfield
        // startup (sqlx VersionMismatch). It must live in its own version, and
        // 0001 must not carry it.
        assert_eq!(migrations[1].version, 2);
        assert!(migrations[1]
            .sql
            .as_str()
            .contains("CREATE TABLE git_repo_names"));
        assert!(!migrations[0].sql.as_str().contains("git_repo_names"));

        // Same additive-migration rule for the per-community workspace icon
        // (NIP-11 `icon`): its own version, never folded into 0001.
        assert_eq!(migrations[2].version, 3);
        assert!(migrations[2]
            .sql
            .as_str()
            .contains("ALTER TABLE communities ADD COLUMN icon"));
        assert!(!migrations[0].sql.as_str().contains("icon"));
        // Same additive-migration rule for the e-tag containment GIN index
        // (channel-window aux closure): its own version, never folded into 0001.
        assert_eq!(migrations[3].version, 4);
        assert!(migrations[3]
            .sql
            .as_str()
            .contains("CREATE INDEX idx_events_tags_gin"));
        assert!(!migrations[0].sql.as_str().contains("idx_events_tags_gin"));

        // NIP-AM (kind 44200) FTS exclusion: additive migration, never folded
        // into 0001 — folding would change 0001's checksum and break brownfield
        // startup. Migration 5 drops and re-adds the generated `search_tsv`
        // column with the extended kind-44200 exclusion. 0001 must NOT carry 44200.
        assert_eq!(migrations[4].version, 5);
        assert!(migrations[4].sql.as_str().contains("search_tsv"));
        assert!(migrations[4].sql.as_str().contains("44200"));
        assert!(!migrations[0].sql.as_str().contains("44200"));

        // Community moderation (reports/bans/audit): additive migration, never
        // folded into 0001 — same brownfield checksum rule as above.
        assert_eq!(migrations[5].version, 6);
        assert!(migrations[5]
            .sql
            .as_str()
            .contains("CREATE TABLE moderation_reports"));
        assert!(migrations[5]
            .sql
            .as_str()
            .contains("CREATE TABLE community_bans"));
        assert!(migrations[5]
            .sql
            .as_str()
            .contains("CREATE TABLE moderation_actions"));
        for action in crate::moderation::MODERATION_ACTION_CHECK_VOCAB {
            assert!(
                migrations[5].sql.as_str().contains(&format!("'{action}'")),
                "migration 0006 moderation_actions.action CHECK must allow {action}"
            );
        }
        assert!(!migrations[0].sql.as_str().contains("moderation_reports"));
        // NIP-RS retention is additive and boot-safe: seed replay watermarks
        // before deleting payload history, without rewriting search storage.
        assert_eq!(migrations[6].version, 7);
        assert!(migrations[6]
            .sql
            .as_str()
            .contains("LOCK TABLE events IN SHARE ROW EXCLUSIVE MODE"));
        assert!(migrations[6]
            .sql
            .as_str()
            .contains("CREATE TABLE parameterized_event_watermarks"));
        assert!(migrations[6]
            .sql
            .as_str()
            .contains("INSERT INTO parameterized_event_watermarks"));
        assert!(migrations[6]
            .sql
            .as_str()
            .contains("CREATE INDEX idx_event_mentions_community_event"));
        assert!(migrations[6]
            .sql
            .as_str()
            .contains("NIP-RS retention blocked: deleted event outranks live head"));
        assert!(migrations[6]
            .sql
            .as_str()
            .contains("DELETE FROM events old"));
        assert!(!migrations[6]
            .sql
            .as_str()
            .contains("ALTER TABLE events DROP COLUMN search_tsv"));

        // Fresh installs opt into the positive search allowlist without making
        // populated databases rewrite their events heap during relay startup.
        assert_eq!(migrations[7].version, 8);
        assert!(migrations[7]
            .sql
            .as_str()
            .contains("IF NOT EXISTS (SELECT 1 FROM events LIMIT 1)"));
        assert!(migrations[7]
            .sql
            .as_str()
            .contains("CASE WHEN kind IN (0, 9, 40002, 45001, 45003)"));
        assert!(migrations[7].sql.as_str().contains("ELSE NULL::tsvector"));

        // Mixed-version guards are additive because 0007/0008 may already be
        // recorded by a running relay and their sqlx checksums are immutable.
        assert_eq!(migrations[8].version, 9);
        assert!(migrations[8]
            .sql
            .as_str()
            .contains("CREATE TRIGGER trg_events_nip_rs_watermark"));
        assert!(migrations[8]
            .sql
            .as_str()
            .contains("stale NIP-RS event rejected by durable watermark"));
        assert!(migrations[8]
            .sql
            .as_str()
            .contains("CREATE TRIGGER trg_events_purge_soft_deleted_nip_rs"));
        assert!(migrations[8]
            .sql
            .as_str()
            .contains("CREATE TRIGGER trg_event_mentions_require_live_event"));

        assert_eq!(migrations[9].version, 10);
        assert!(migrations[9]
            .sql
            .as_str()
            .contains("CREATE OR REPLACE FUNCTION guard_nip_rs_watermark"));
        assert!(migrations[9].sql.as_str().contains("RETURN NULL"));

        assert_eq!(migrations[10].version, 11);
        assert!(migrations[10]
            .sql
            .as_str()
            .contains("CREATE OR REPLACE FUNCTION guard_nip_rs_watermark"));
        assert!(migrations[10]
            .sql
            .as_str()
            .contains("CREATE OR REPLACE FUNCTION purge_soft_deleted_nip_rs"));
        assert!(migrations[10].sql.as_str().contains("tag->>0 = 'd'"));
        assert!(migrations[10].sql.as_str().contains(") = 1"));

        // Push leases and their durable outbox are relay-owned and structurally
        // community-scoped; the public gateway remains stateless.
        assert_eq!(migrations[11].version, 12);
        assert!(migrations[11]
            .sql
            .as_str()
            .contains("CREATE TABLE push_leases"));
        assert!(migrations[11]
            .sql
            .as_str()
            .contains("CREATE TABLE push_wake_outbox"));
        assert!(migrations[11]
            .sql
            .as_str()
            .contains("PRIMARY KEY (community_id, author, installation_id)"));
        assert!(!migrations[0].sql.as_str().contains("push_leases"));

        assert_eq!(migrations[12].version, 13);
        assert!(migrations[12]
            .sql
            .as_str()
            .contains("ADD COLUMN endpoint_enabled"));

        // Kind 30350 is author-only encrypted data, so its ciphertext is never
        // indexed for NIP-50 search. Preserve the 0001 checksum and extend the
        // generated expression additively.
        assert_eq!(migrations[13].version, 14);
        assert!(migrations[13].sql.as_str().contains("30350"));
        assert!(migrations[13].sql.as_str().contains("search_tsv"));
        assert!(!migrations[0].sql.as_str().contains("30350"));

        // Public push-gateway authority is intentionally deployment-global and
        // durable: immediate revocation and hostile-relay admission cannot be
        // honestly provided by a stateless gateway.
        assert_eq!(migrations[14].version, 15);
        assert!(migrations[14]
            .sql
            .as_str()
            .contains("CREATE TABLE push_gateway_installations"));
        assert!(migrations[14]
            .sql
            .as_str()
            .contains("push_gateway_delegations"));
        assert!(migrations[14]
            .sql
            .as_str()
            .contains("_operator_global_tables"));

        // Community archival and product feedback landed concurrently. Keep
        // both additive migrations in a single, unambiguous sequence.
        assert_eq!(migrations[15].version, 16);
        assert!(migrations[15]
            .sql
            .as_str()
            .contains("ADD COLUMN archived_at"));

        // Product feedback is a deployment-private sidecar; community_id is
        // provenance, not an operator-review authorization boundary.
        assert_eq!(migrations[16].version, 17);
        assert!(migrations[16]
            .sql
            .as_str()
            .contains("CREATE TABLE product_feedback"));
        assert!(migrations[16]
            .sql
            .as_str()
            .contains("community_id UUID NOT NULL"));
        assert!(migrations[16]
            .sql
            .as_str()
            .contains("('product_feedback', 'deployment product inbox"));
        assert!(!migrations[0].sql.as_str().contains("product_feedback"));

        // Matching is driven from a parent-table trigger so all partition and
        // internal insertion paths share the same crash-safe allowlist seam.
        assert_eq!(migrations[17].version, 18);
        let matcher = migrations[17].sql.as_str();
        assert!(matcher.contains("CREATE TABLE push_match_queue"));
        assert!(matcher.contains("AFTER INSERT ON events"));
        assert!(matcher.contains("NEW.kind IN (7, 9, 1059, 40007, 46010)"));
        assert!(!migrations[0].sql.as_str().contains("push_match_queue"));

        // Mesh status is a heartbeat, not an audit stream. The additive
        // migration removes accumulated soft-deleted payloads and covers old
        // writers during rolling deploys without changing kind:30003 broadly.
        assert_eq!(migrations[18].version, 19);
        let mesh_retention = migrations[18].sql.as_str();
        assert!(mesh_retention.contains("buzz-mesh-member-status:%"));
        assert!(mesh_retention.contains("buzz-mesh-status"));
        assert!(mesh_retention
            .contains("CREATE TRIGGER trg_events_purge_soft_deleted_buzz_mesh_status"));
        assert!(!migrations[0]
            .sql
            .as_str()
            .contains("purge_soft_deleted_buzz_mesh_status"));

        // Join policy acceptances landed concurrently with mesh status retention;
        // keep both additive migrations in a single, unambiguous sequence.
        assert_eq!(migrations[19].version, 20);
        assert!(migrations[19]
            .sql
            .as_str()
            .contains("CREATE TABLE join_policy_acceptances"));

        // Replica-fence commit-time floor guard on channel-bearing events.
        assert_eq!(migrations[20].version, 21);
        assert!(migrations[20]
            .sql
            .as_str()
            .contains("events_created_at_floor_guard"));
        assert!(!migrations[0]
            .sql
            .as_str()
            .contains("join_policy_acceptances"));

        // Channel TTL refresh belongs to the event insertion transaction so a
        // concurrent permanent -> ephemeral transition cannot be missed.
        assert_eq!(migrations[21].version, 22);
        let ttl_refresh = migrations[21].sql.as_str();
        assert!(ttl_refresh.contains("CREATE CONSTRAINT TRIGGER events_refresh_channel_ttl"));
        assert!(ttl_refresh.contains("AFTER INSERT ON events"));
        assert!(ttl_refresh.contains("DEFERRABLE INITIALLY DEFERRED"));
        assert!(ttl_refresh.contains("clock_timestamp()"));
        assert!(ttl_refresh.contains("NEW.kind <> 9007"));

        // T1b push gate: the match-queue trigger only enqueues when the
        // community has an eligible lease, ordered against lease activations
        // through the shared/exclusive per-community advisory lock.
        assert_eq!(migrations[22].version, 23);
        let push_gate = migrations[22].sql.as_str();
        assert!(push_gate.contains("CREATE OR REPLACE FUNCTION enqueue_push_match_job"));
        assert!(push_gate.contains("pg_advisory_xact_lock_shared"));
        assert!(push_gate.contains("'buzz_push_gate:' || NEW.community_id::text"));
        assert!(push_gate.contains("endpoint_enabled"));

        // T1a repair: the TTL refresh trigger synchronizes on a shared
        // per-channel advisory lock instead of FOR UPDATE on the channel row,
        // so permanent-channel commits no longer serialize.
        assert_eq!(migrations[23].version, 24);
        let ttl_shared = migrations[23].sql.as_str();
        assert!(ttl_shared
            .contains("CREATE OR REPLACE FUNCTION refresh_channel_ttl_after_event_insert"));
        assert!(ttl_shared.contains("pg_advisory_xact_lock_shared"));
        assert!(ttl_shared.contains("'buzz_channel_ttl:' || NEW.community_id::text"));
        // The row read must be a bare SELECT (comments describe the removed
        // FOR UPDATE; the executable body must not reintroduce it).
        assert!(ttl_shared.contains("SELECT ttl_seconds INTO channel_ttl"));
        assert!(!strip_sql_comments(ttl_shared)
            .to_lowercase()
            .contains("for update"));
        assert!(ttl_shared.contains("NEW.kind <> 9007"));

        // Project View canonical state is additive and disabled by default.
        // The schema, tenant-leading keys, active-count trigger, and deferred
        // aggregate guard all land together so no relay can observe a partial
        // storage contract.
        assert_eq!(migrations[24].version, 25);
        let project_view = migrations[24].sql.as_str();
        assert!(project_view.contains("ADD COLUMN project_view_enabled"));
        assert!(project_view.contains("DEFAULT FALSE"));
        assert!(project_view.contains("CREATE TABLE project_view_state"));
        assert!(project_view.contains("CREATE TABLE project_view_objects"));
        assert!(project_view.contains("CREATE TABLE project_view_mutations"));
        assert!(project_view.contains("PRIMARY KEY (community_id, object_id)"));
        assert!(project_view.contains("PRIMARY KEY (community_id, event_id)"));
        assert!(project_view.contains("DEFERRABLE INITIALLY DEFERRED"));
        assert!(project_view.contains("project_view_adjust_active_count"));
        assert!(project_view.contains("project_view_validate_aggregate"));
        assert!(project_view.contains("project_view_validate_object"));
        assert!(!project_view.contains("SELECT count(*) INTO actual_count"));
        assert!(!migrations[0].sql.as_str().contains("project_view_objects"));

        // Role continuity is another additive, disabled-by-default layer.
        // Community schema version 1 remains the default, while the v2 tables,
        // partial uniqueness constraints, and deferred cross-table guard land
        // before any Community can be cut over.
        assert_eq!(migrations[25].version, 26);
        let role_continuity = migrations[25].sql.as_str();
        assert!(role_continuity.contains("ADD COLUMN project_view_schema_version"));
        assert!(role_continuity.contains("DEFAULT 1"));
        assert!(role_continuity.contains("CREATE TABLE project_view_changes"));
        assert!(role_continuity.contains("CREATE TABLE project_role_assignments"));
        assert!(role_continuity.contains("CREATE TABLE project_role_assignment_proposals"));
        assert!(role_continuity.contains("CREATE TABLE project_work_commitments"));
        assert!(role_continuity.contains("CREATE TABLE project_role_checkpoints"));
        assert!(role_continuity.contains("CREATE TABLE project_role_handoffs"));
        assert!(role_continuity.contains("CREATE TABLE project_role_continuity_references"));
        assert!(role_continuity
            .contains("CREATE UNIQUE INDEX idx_project_role_assignments_active_role"));
        assert!(role_continuity
            .contains("CREATE UNIQUE INDEX idx_project_role_assignments_active_member"));
        assert!(role_continuity.contains("DEFERRABLE INITIALLY DEFERRED"));
        assert!(role_continuity.contains("project_role_continuity_validate_community"));

        // Stage 2 adds the lifecycle fields and materialized counts needed by
        // the first live v2 Proposal / Assignment coordinator.
        assert_eq!(migrations[26].version, 27);
        let role_assignment_state = migrations[26].sql.as_str();
        assert!(role_assignment_state.contains("ADD COLUMN open_proposal_count"));
        assert!(role_assignment_state.contains("ADD COLUMN entity_revision"));
        assert!(role_assignment_state.contains("ADD COLUMN replaced_by_assignment_id"));
        assert!(role_assignment_state.contains("project_role_continuity_validate_counts"));

        // Stage 5 promotes the reserved Commitment relation into a complete
        // projected lifecycle entity with immutable Member attribution.
        assert_eq!(migrations[27].version, 28);
        let work_commitments = migrations[27].sql.as_str();
        assert!(work_commitments.contains("ADD COLUMN member_pubkey"));
        assert!(work_commitments.contains("ADD COLUMN entity_revision"));
        assert!(work_commitments.contains("ADD COLUMN last_change_id"));
        assert!(work_commitments.contains("project_work_commitments_validate_stage5_community"));

        // Stage 6 activates the reserved append-only continuity history
        // tables and validates attribution and typed references at commit.
        assert_eq!(migrations[28].version, 29);
        let role_history = migrations[28].sql.as_str();
        assert!(role_history.contains("ADD COLUMN based_on_project_revision"));
        assert!(role_history.contains("ADD COLUMN checkpoint_id"));
        assert!(role_history.contains("project_role_history_append_only"));
        assert!(role_history.contains("project_role_history_validate_stage6_community"));
        assert!(
            role_history.contains("source_change.operation IS DISTINCT FROM 'append_checkpoint'")
        );
        assert!(role_history.contains("source_change.operation IS DISTINCT FROM 'append_handoff'"));

        // Stage 7 keeps high-frequency runtime state outside Project revisions,
        // while deferring the terminal trust-chain validation until the atomic
        // system change is complete.
        assert_eq!(migrations[29].version, 30);
        let runtime_supervision = migrations[29].sql.as_str();
        assert!(runtime_supervision.contains("CREATE TABLE project_runtime_supervisor_bindings"));
        assert!(runtime_supervision.contains("CREATE TABLE project_runtime_leases"));
        assert!(runtime_supervision.contains("CREATE TABLE project_runtime_evidence"));
        assert!(runtime_supervision.contains("recovery_backoff_seconds"));
        assert!(runtime_supervision.contains("recovery_attempt_in_flight"));
        assert!(runtime_supervision.contains("next_recovery_at"));
        assert!(runtime_supervision.contains("project_runtime_evidence_append_only"));
        assert!(runtime_supervision.contains("project_runtime_supervision_validate_community"));
        assert!(runtime_supervision
            .contains("Runtime lease is not backed by its exact latest evidence"));
        assert!(runtime_supervision.contains("Runtime evidence does not match its trusted binding"));
        assert!(runtime_supervision.contains("DEFERRABLE INITIALLY DEFERRED"));

        // The concrete supervisor adapter needs a non-failure terminal state
        // for deliberate Desktop/ACP shutdowns.
        assert_eq!(migrations[30].version, 31);
        let graceful_stop = migrations[30].sql.as_str();
        assert!(graceful_stop.contains("project_runtime_evidence_type_check"));
        assert!(graceful_stop.contains("'graceful_stop'"));

        // Project Document lands as one additive, flag-off canonical kernel.
        assert_eq!(migrations[31].version, 32);
        let project_document = migrations[31].sql.as_str();
        assert!(project_document.contains("ADD COLUMN project_document_enabled"));
        assert!(project_document.contains("DEFAULT FALSE"));
        assert!(project_document.contains("CREATE TABLE project_document_state"));
        assert!(project_document.contains("CREATE TABLE project_documents"));
        assert!(project_document.contains("CREATE TABLE project_document_revisions"));
        assert!(project_document.contains("CREATE TABLE project_document_changes"));
        assert!(project_document.contains("DEFERRABLE INITIALLY DEFERRED"));
        assert!(project_document.contains("project_document_revisions_append_only"));
        assert!(project_document.contains("project_document_reject_hard_delete"));
        assert!(project_document.contains("project_document_validate_community"));

        // Project View v3 lands additively and keeps every capability off.
        assert_eq!(migrations[32].version, 33);
        let project_view_v3 = migrations[32].sql.as_str();
        assert!(project_view_v3.contains("ADD COLUMN project_context_enabled"));
        assert!(project_view_v3.contains("DEFAULT FALSE"));
        assert!(project_view_v3.contains("CREATE TABLE project_view_object_provenance"));
        assert!(project_view_v3.contains("CREATE TABLE project_view_resource_context_references"));
        assert!(project_view_v3.contains("CREATE TABLE project_view_document_context_references"));
        assert!(project_view_v3.contains("CREATE TABLE project_view_v3_resource_mappings"));
        assert!(project_view_v3.contains("CREATE TABLE project_view_maintenance_epochs"));
        assert!(project_view_v3.contains("CREATE TABLE project_view_provisioning_operations"));
        assert!(project_view_v3
            .contains("CREATE OR REPLACE FUNCTION project_role_continuity_validate_community"));
        assert!(project_view_v3.contains("project_view_v3_validate_community"));

        // Project Context gets a separate, append-only, replay-first control
        // ledger. Merely applying the migration cannot enable the capability.
        assert_eq!(migrations[33].version, 34);
        let project_context_control = migrations[33].sql.as_str();
        assert!(project_context_control.contains("CREATE TABLE project_view_context_operations"));
        assert!(project_context_control.contains("UNIQUE (community_id, idempotency_key_hash)"));
        assert!(project_context_control.contains("closure_protocol_version"));
        assert!(project_context_control.contains("project_view_context_operations_immutable"));
        assert!(!project_context_control.contains("SET project_context_enabled = TRUE"));

        // Full-history Document reprojection stages outside `events` and keeps
        // the capability disabled until an explicit later enable.
        assert_eq!(migrations[34].version, 35);
        let project_document_reproject = migrations[34].sql.as_str();
        assert!(project_document_reproject.contains("CREATE TABLE project_document_reprojects"));
        assert!(
            project_document_reproject.contains("CREATE TABLE project_document_reproject_events")
        );
        assert!(project_document_reproject.contains("project_document_validate_history_projection"));
        assert!(!project_document_reproject.contains("project_document_enabled = TRUE"));

        // Open Proposals must never outlive their active Role. The migration
        // validates existing v2/v3 Communities and installs deferred guards on
        // both sides of that cross-domain reference.
        assert_eq!(migrations[35].version, 36);
        let role_proposal_guard = migrations[35].sql.as_str();
        assert!(role_proposal_guard.contains("project_role_open_proposal_validate_community"));
        assert!(role_proposal_guard.contains("project_view_objects_open_proposal_role_validate"));
        assert!(role_proposal_guard.contains("project_role_proposals_role_validate"));
        assert!(role_proposal_guard.contains("proposal.status = 'open'"));
        assert!(role_proposal_guard.contains("role_object.deleted_at IS NOT NULL"));
        assert!(role_proposal_guard.contains("DEFERRABLE INITIALLY DEFERRED"));

        // Meeting V0 lifecycle projection follows the complete Project View
        // series so every migration version remains globally unique.
        assert_eq!(migrations[36].version, 37);
        let meeting_v0 = migrations[36].sql.as_str();
        assert!(meeting_v0.contains("ADD COLUMN room_kind"));
        assert!(meeting_v0.contains("CREATE TABLE meeting_sessions"));
        assert!(meeting_v0.contains("PRIMARY KEY (community_id, session_id)"));
        assert!(!migrations[0].sql.as_str().contains("meeting_sessions"));

        // Meeting V0 stage 2 persists the complete floor state machine and its
        // transactional delivery outbox in the next additive migration.
        assert_eq!(migrations[37].version, 38);
        let meeting_floor = migrations[37].sql.as_str();
        assert!(meeting_floor.contains("CREATE TABLE meeting_rounds"));
        assert!(meeting_floor.contains("CREATE TABLE meeting_floor_claims"));
        assert!(meeting_floor.contains("CREATE TABLE meeting_event_outbox"));
        assert!(meeting_floor
            .contains("PRIMARY KEY (community_id, session_id, round_number, claimant_pubkey)"));

        // Meeting V0 stage 3 adds agent decision signals and enables early
        // cohort settlement without changing either prior Meeting migration.
        assert_eq!(migrations[38].version, 39);
        let meeting_agent_floor = migrations[38].sql.as_str();
        assert!(meeting_agent_floor.contains("CREATE TABLE meeting_floor_signals"));
        assert!(meeting_agent_floor.contains("action IN ('ready', 'pass', 'yield')"));
        assert!(meeting_agent_floor.contains("CREATE TABLE meeting_round_decision_cohort"));
        assert!(meeting_agent_floor
            .contains("PRIMARY KEY (community_id, session_id, round_number, participant_pubkey)"));

        // Meeting V1 remains additive and persists a policy-isolated,
        // recoverable moderated-baton projection.
        assert_eq!(migrations[39].version, 40);
        let meeting_baton = migrations[39].sql.as_str();
        assert!(meeting_baton.contains("moderated-baton-v1"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_participants"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_baton_config"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_baton_state_history"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_baton_state"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_speech_intents"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_human_floor_requests"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_baton_offers"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_baton_grants"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_grant_progress"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_directed_handoffs"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_baton_fallback_attempts"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_v1_command_receipts"));
        assert!(meeting_baton.contains("CREATE TABLE meeting_revocation_jobs"));
        assert!(meeting_baton.contains("chk_meeting_protocol_shape"));
        assert!(meeting_baton.contains("trg_meeting_session_protocol_immutable"));

        // Stage 2 corrects the deterministic Progress vocabulary without
        // rewriting the checksum-frozen Meeting V1 foundation migration.
        assert_eq!(migrations[40].version, 41);
        let meeting_baton_stage_two = migrations[40].sql.as_str();
        assert!(meeting_baton_stage_two.contains("'context_sync'"));
        assert!(meeting_baton_stage_two.contains("'tool_use'"));
        assert!(meeting_baton_stage_two.contains("'generating'"));
        assert!(meeting_baton_stage_two.contains("'submitting'"));
        assert!(
            meeting_baton_stage_two.contains("idx_meeting_event_outbox_pending_session_sequence")
        );
        assert!(meeting_baton_stage_two.contains("recovery_retry_at"));
        assert!(meeting_baton_stage_two.contains("recovery_attempts"));
        assert!(meeting_baton_stage_two.contains("idx_meeting_baton_state_recovery_due"));
        assert!(meeting_baton_stage_two.contains("idx_meeting_revocation_jobs_reader_fence"));

        // Moderator optimistic-decision state remains additive: candidate
        // eligibility, signed Attempt snapshots, and one-use retry evidence
        // upgrade existing V1 sessions without rewriting prior migrations.
        assert_eq!(migrations[41].version, 42);
        let meeting_moderator_attempts = migrations[41].sql.as_str();
        assert!(meeting_moderator_attempts.contains("eligible_decision_epoch"));
        assert!(
            meeting_moderator_attempts.contains("CREATE TABLE meeting_moderator_decision_attempts")
        );
        assert!(meeting_moderator_attempts.contains("CREATE TABLE meeting_moderator_retry_tickets"));
        assert!(meeting_moderator_attempts.contains("active_decision_attempt_id"));
        assert!(meeting_moderator_attempts.contains("retry_ticket_id"));

        // Meeting V2 stage one adds a protocol-isolated current board while
        // deliberately keeping its runtime fail-closed.
        assert_eq!(migrations[42].version, 43);
        let meeting_v2_stage_one = migrations[42].sql.as_str();
        assert!(meeting_v2_stage_one.contains("moderated-board-v1"));
        assert!(meeting_v2_stage_one.contains("CREATE TABLE meeting_current_boards"));
        assert!(meeting_v2_stage_one.contains("CREATE TABLE meeting_v2_bootstrap_state"));
        assert!(meeting_v2_stage_one.contains("bootstrap_locked"));

        // Meeting V2 stage two adds the Board/Floor gate, command receipts,
        // independent Board timing, and explicit closed/aborted outcomes.
        assert_eq!(migrations[43].version, 44);
        let meeting_v2_stage_two = migrations[43].sql.as_str();
        assert!(meeting_v2_stage_two.contains("CREATE TABLE meeting_v2_config"));
        assert!(meeting_v2_stage_two.contains("board_pending"));
        assert!(meeting_v2_stage_two.contains("floor_ready"));
        assert!(meeting_v2_stage_two.contains("CREATE TABLE meeting_v2_board_command_receipts"));
        assert!(meeting_v2_stage_two.contains("terminal_outcome"));

        // Action finalization is an additive, policy-discriminated Meeting V2
        // lifecycle stage with its own durable saga ledger.
        assert_eq!(migrations[44].version, 45);
        let meeting_v2_actions = migrations[44].sql.as_str();
        assert!(meeting_v2_actions.contains("moderated-board-actions-v1"));
        assert!(meeting_v2_actions.contains("finalizing_actions"));
        assert!(meeting_v2_actions.contains("CREATE TABLE meeting_v2_action_runs"));
        assert!(meeting_v2_actions.contains("CREATE TABLE meeting_v2_action_steps"));
        assert!(meeting_v2_actions.contains("CREATE TABLE meeting_v2_action_command_receipts"));

        // Direct action finalization removes the unreleased Plan/Step runtime
        // and replaces it with a moderator attestation fence.
        assert_eq!(migrations[45].version, 46);
        let meeting_v2_direct_actions = migrations[45].sql.as_str();
        assert!(meeting_v2_direct_actions.contains("moderated-board-actions-v2"));
        assert!(meeting_v2_direct_actions.contains("DROP TABLE meeting_v2_action_steps"));
        assert!(meeting_v2_direct_actions.contains("completion_event_id"));
        assert!(meeting_v2_direct_actions
            .contains("action IN ('begin', 'block', 'retry', 'return-to-board')"));

        // Renewable action leases are a one-shot v3 cutover. Ended v2 rows
        // remain history while all new runnable action Meetings use v3.
        assert_eq!(migrations[46].version, 47);
        let meeting_v2_action_leases = migrations[46].sql.as_str();
        assert!(meeting_v2_action_leases.contains("moderated-board-actions-v3"));
        assert!(meeting_v2_action_leases.contains("progress_seq"));
        assert!(meeting_v2_action_leases.contains("operator_hard_deadline"));
        assert!(meeting_v2_action_leases.contains("CREATE TABLE meeting_v2_action_lease_renewals"));
        assert!(meeting_v2_action_leases
            .contains("action IN ('begin', 'renew', 'block', 'retry', 'return-to-board')"));

        // Greenfield Communities start on the only ordinary runtime schema
        // major. Existing rows are intentionally untouched and retain their
        // explicit migration/recovery coordinate.
        assert_eq!(migrations[47].version, 48);
        let project_view_v3_greenfield_default = migrations[47].sql.as_str();
        assert!(project_view_v3_greenfield_default
            .contains("ALTER COLUMN project_view_schema_version SET DEFAULT 3"));
        for required in [
            "CREATE FUNCTION project_view_v3_bootstrap_lifecycle_valid",
            "CREATE OR REPLACE FUNCTION project_view_v3_validate_row",
            "CREATE OR REPLACE FUNCTION project_role_continuity_validate_community",
            "maintenance.state = 'normal'",
            "maintenance.current_epoch IS NULL",
            "project_view_maintenance_epochs epoch",
            "project_view_v3_resource_mappings mapping",
            "project_view_context_operations context_operation",
            "project_view_provisioning_operations preparation",
            "project_role_continuity_references reference",
        ] {
            assert!(
                project_view_v3_greenfield_default.contains(required),
                "migration 0048 must preserve the v3 greenfield invariant {required}"
            );
        }
        assert!(!project_view_v3_greenfield_default.contains("UPDATE communities"));
        assert!(!project_view_v3_greenfield_default.contains("DELETE FROM communities"));

        // Project Context Edge lands as a separate capability-off canonical
        // kernel. The migration owns normalized coordinate identity, durable
        // binding tombstones, replay receipts, and commit-time parity guards.
        assert_eq!(migrations[48].version, 49);
        let project_context_edge = migrations[48].sql.as_str();
        assert!(project_context_edge.contains("ADD COLUMN project_context_edge_enabled"));
        assert!(project_context_edge.contains("DEFAULT FALSE"));
        assert!(project_context_edge.contains("CREATE TABLE project_context_edge_state"));
        assert!(project_context_edge.contains("CREATE TABLE project_context_edges"));
        assert!(project_context_edge.contains("CREATE TABLE project_context_edge_coordinates"));
        assert!(project_context_edge.contains("CREATE TABLE project_context_document_bindings"));
        assert!(project_context_edge.contains("CREATE TABLE project_context_edge_changes"));
        assert!(project_context_edge.contains("project_context_compute_edge_key"));
        assert!(project_context_edge.contains("project_context_validate_community"));
        assert!(project_context_edge.contains("project_context_validate_new_change"));
        assert!(project_context_edge.contains("idx_project_context_bindings_active_document"));
        assert!(project_context_edge.contains("DEFERRABLE INITIALLY DEFERRED"));
        assert!(project_context_edge.contains("projection.kind = 40908"));
        assert!(project_context_edge.contains("event.kind = 40909"));
        assert!(project_context_edge.contains("command.kind = 44302"));
        assert!(project_context_edge.contains("buzz.project_context_reproject"));
        assert!(
            project_context_edge.contains("Project Context revision-zero metadata must be a reset")
        );
        assert!(!project_context_edge.contains("SET project_context_edge_enabled = TRUE"));

        // Keep the checksum-frozen Project Context foundation migration intact
        // while making its edge-key guard usable by the fresh-schema path,
        // where pgcrypto is not installed by the schema planner.
        assert_eq!(migrations[49].version, 50);
        let project_context_builtin_sha256 = migrations[49].sql.as_str();
        assert!(project_context_builtin_sha256
            .contains("CREATE OR REPLACE FUNCTION project_context_compute_edge_key"));
        assert!(project_context_builtin_sha256.contains("RETURN sha256(payload)"));

        assert_eq!(migrations[50].version, 51);
        let project_context_meeting_v2 = migrations[50].sql.as_str();
        for required in [
            "schema_version IN (1, 2)",
            "coordinate_type = 'meeting'",
            "project_context_meeting_is_terminal",
            "WHEN 'meeting' THEN",
            "context_state.schema_version = 2",
            "OLD.schema_version = 1 AND NEW.schema_version = 2",
        ] {
            assert!(
                project_context_meeting_v2.contains(required),
                "migration 0051 must contain {required}"
            );
        }
        assert!(!project_context_meeting_v2.contains("DELETE FROM project_context"));

        assert_eq!(migrations[51].version, 52);
        let meeting_community_read = migrations[51].sql.as_str();
        for required in [
            "ADD COLUMN meeting_community_read_enabled",
            "ADD COLUMN meeting_community_read_create_paused",
            "legacy_meeting_visibility_watermark",
            "legacy_meeting_visibility_audit_digest",
            "legacy_meeting_visibility_approved_at",
            "legacy_meeting_visibility_approved_by",
            "meeting_community_read_contract_immutable",
        ] {
            assert!(
                meeting_community_read.contains(required),
                "migration 0052 must contain {required}"
            );
        }
        for destructive in [
            "DELETE FROM",
            "TRUNCATE",
            "DROP TABLE",
            "DROP COLUMN",
            "UPDATE meeting_sessions",
            "UPDATE events",
        ] {
            assert!(
                !meeting_community_read.contains(destructive),
                "migration 0052 must not contain destructive statement {destructive}"
            );
        }

        assert_eq!(migrations[52].version, 53);
        let project_context_meeting_read = migrations[52].sql.as_str();
        for required in [
            "project_context_requires_meeting_community_read",
            "project_context_edge_enabled",
            "meeting_community_read_enabled",
        ] {
            assert!(
                project_context_meeting_read.contains(required),
                "migration 0053 must contain {required}"
            );
        }
        for destructive in ["DELETE FROM", "TRUNCATE", "DROP TABLE", "DROP COLUMN"] {
            assert!(
                !project_context_meeting_read.contains(destructive),
                "migration 0053 must not contain destructive statement {destructive}"
            );
        }

        assert_eq!(migrations[53].version, 54);
        let project_context_finalizing_meeting = migrations[53].sql.as_str();
        for required in [
            "project_context_meeting_is_attachable",
            "meeting_v2_action_command_receipts",
            "action_window_epoch = 1",
            "action_finalization_began",
            "board_projection.pubkey = context_state.projection_pubkey",
            "state_projection.pubkey = context_state.projection_pubkey",
        ] {
            assert!(
                project_context_finalizing_meeting.contains(required),
                "migration 0054 must contain {required}"
            );
        }
        for destructive in [
            "DELETE FROM",
            "TRUNCATE",
            "DROP TABLE",
            "DROP COLUMN",
            "UPDATE ",
        ] {
            assert!(
                !project_context_finalizing_meeting.contains(destructive),
                "migration 0054 must not contain destructive statement {destructive}"
            );
        }

        assert_eq!(migrations[54].version, 55);
        let local_membership_recovery = migrations[54].sql.as_str();
        for required in [
            "project_view_v3_membership_snapshot_recoveries",
            "canonical_request_hash",
            "restored_membership_event_id",
            "retired_membership_event_id",
            "project_view_v3_reject_ledger_mutation",
        ] {
            assert!(
                local_membership_recovery.contains(required),
                "migration 0055 must contain {required}"
            );
        }
        for destructive in [
            "DELETE FROM",
            "TRUNCATE",
            "DROP TABLE",
            "DROP COLUMN",
            "UPDATE events",
            "UPDATE project_view_state",
        ] {
            assert!(
                !local_membership_recovery.contains(destructive),
                "migration 0055 must not contain destructive statement {destructive}"
            );
        }
    }

    #[test]
    fn desired_schema_contains_project_context_edge_storage() {
        let schema = include_str!("../../../schema/schema.sql");

        for required in [
            "project_context_edge_enabled BOOLEAN NOT NULL DEFAULT FALSE",
            "CREATE TABLE project_context_edge_state",
            "CREATE TABLE project_context_edges",
            "CREATE TABLE project_context_edge_coordinates",
            "CREATE TABLE project_context_document_bindings",
            "CREATE TABLE project_context_edge_changes",
            "project_context_edges_exact_set_unique",
            "project_context_compute_edge_key",
            "project_context_validate_community",
            "project_context_validate_new_change",
            "project_context_meeting_is_terminal",
            "project_context_meeting_is_attachable",
            "coordinate_type = 'meeting'",
            "schema_version IN (1, 2)",
            "context_state.schema_version = 2",
            "active Context Document must be detached before deletion",
            "idx_project_context_edge_coordinates_lookup",
            "idx_project_context_bindings_active_document",
            "projection.kind = 40908",
            "event.kind = 40909",
            "command.kind = 44302",
            "buzz.project_context_reproject",
            "meeting_community_read_enabled BOOLEAN NOT NULL DEFAULT FALSE",
            "meeting_community_read_create_paused BOOLEAN NOT NULL DEFAULT FALSE",
            "legacy_meeting_visibility_watermark BIGINT",
            "legacy_meeting_visibility_audit_digest BYTEA",
            "meeting_community_read_contract_immutable",
            "project_context_requires_meeting_community_read",
        ] {
            assert!(
                schema.contains(required),
                "schema/schema.sql must include migration 0049 object {required}"
            );
        }
    }

    #[test]
    fn desired_schema_contains_moderator_optimistic_decision_state() {
        let schema = include_str!("../../../schema/schema.sql");

        for required in [
            "moderator_max_rejudgments",
            "moderator_max_cas_rebases_per_attempt",
            "eligible_decision_epoch",
            "active_decision_attempt_id",
            "CREATE TABLE meeting_moderator_decision_attempts",
            "CREATE TABLE meeting_moderator_retry_tickets",
            "fk_meeting_baton_active_decision_attempt",
            "fk_meeting_v1_receipt_retry_ticket",
        ] {
            assert!(
                schema.contains(required),
                "schema/schema.sql must include migration 0042 object {required}"
            );
        }
    }

    #[test]
    fn desired_schema_contains_meeting_v2_stage_one_state() {
        let schema = include_str!("../../../schema/schema.sql");

        for required in [
            "moderated-board-v1",
            "CREATE TABLE meeting_current_boards",
            "CREATE TABLE meeting_v2_bootstrap_state",
            "bootstrap_locked",
            "OCTET_LENGTH(board_content) <= 65536",
        ] {
            assert!(
                schema.contains(required),
                "schema/schema.sql must include migration 0043 object {required}"
            );
        }
    }

    #[test]
    fn desired_schema_contains_meeting_v2_stage_two_state() {
        let schema = include_str!("../../../schema/schema.sql");

        for required in [
            "CREATE TABLE meeting_v2_config",
            "board_maintenance_ms",
            "board_window",
            "board_pending",
            "floor_ready",
            "CREATE TABLE meeting_v2_board_command_receipts",
            "terminal_outcome",
        ] {
            assert!(
                schema.contains(required),
                "schema/schema.sql must include migration 0044 object {required}"
            );
        }
    }

    #[test]
    fn desired_schema_contains_meeting_v2_direct_action_finalization_state() {
        let schema = include_str!("../../../schema/schema.sql");

        for required in [
            "moderated-board-actions-v3",
            "finalizing_actions",
            "action_finalization_ms",
            "CREATE TABLE meeting_v2_action_runs",
            "completion_event_id",
            "CREATE TABLE meeting_v2_action_command_receipts",
            "action IN ('begin', 'renew', 'block', 'retry', 'return-to-board')",
            "CREATE TABLE meeting_v2_action_lease_renewals",
            "progress_seq",
            "operator_hard_deadline",
        ] {
            assert!(
                schema.contains(required),
                "schema/schema.sql must include migration 0046 object {required}"
            );
        }
        for removed in [
            "CREATE TABLE meeting_v2_action_steps",
            "CREATE TABLE meeting_v2_action_step_attempts",
            "plan_event_id",
            "action_phase",
        ] {
            assert!(
                !schema.contains(removed),
                "schema/schema.sql must omit removed planned-action object {removed}"
            );
        }
    }

    #[test]
    fn checked_in_schema_contains_project_view_migration_state() {
        let schema = include_str!("../../../schema/schema.sql");
        for fragment in [
            "project_view_enabled BOOLEAN NOT NULL DEFAULT FALSE",
            "CREATE TABLE project_view_state",
            "CREATE TABLE project_view_objects",
            "CREATE TABLE project_view_mutations",
            "project_view_objects_adjust_active_count",
            "project_view_validate_object",
            "project_view_state_validate",
            "project_view_objects_validate",
            "ADD COLUMN project_view_schema_version SMALLINT NOT NULL DEFAULT 1",
            "ALTER COLUMN project_view_schema_version SET DEFAULT 3",
            "Project View state missing outside the valid schema-v3 bootstrap lifecycle",
            "CREATE FUNCTION project_view_v3_bootstrap_lifecycle_valid",
            "CREATE OR REPLACE FUNCTION project_view_v3_validate_row",
            "project_view_maintenance_epochs epoch",
            "project_view_v3_resource_mappings mapping",
            "project_view_context_operations context_operation",
            "CREATE TABLE project_view_changes",
            "CREATE TABLE project_role_assignments",
            "idx_project_role_assignments_active_role",
            "idx_project_role_assignments_active_member",
            "project_role_continuity_validate_community",
            "open_proposal_count INTEGER NOT NULL DEFAULT 0",
            "project_role_continuity_validate_counts",
            "project_work_commitments_member_pubkey_check",
            "project_work_commitments_validate_stage5_community",
            "project_role_checkpoints_content_check",
            "project_role_handoffs_content_check",
            "project_role_history_append_only",
            "project_role_history_validate_stage6_community",
            "CREATE TABLE project_runtime_supervisor_bindings",
            "CREATE TABLE project_runtime_leases",
            "CREATE TABLE project_runtime_evidence",
            "recovery_backoff_seconds",
            "recovery_attempt_in_flight",
            "next_recovery_at",
            "project_runtime_evidence_append_only",
            "'graceful_stop'",
            "project_runtime_supervision_validate_community",
            "Runtime lease is not backed by its exact latest evidence",
            "Runtime evidence does not match its trusted binding",
            "project_document_enabled BOOLEAN NOT NULL DEFAULT FALSE",
            "CREATE TABLE project_document_state",
            "CREATE TABLE project_documents",
            "CREATE TABLE project_document_revisions",
            "CREATE TABLE project_document_changes",
            "idx_project_document_revisions_history",
            "project_document_revisions_append_only",
            "project_document_reject_hard_delete",
            "project_document_validate_community",
            "CREATE TABLE project_document_reprojects",
            "CREATE TABLE project_document_reproject_events",
            "project_document_validate_history_projection",
            "CREATE TABLE project_view_context_operations",
            "project_view_context_operations_idempotency_unique",
            "project_view_context_operations_immutable",
            "project_role_open_proposal_validate_community",
            "project_view_objects_open_proposal_role_validate",
            "project_role_proposals_role_validate",
        ] {
            assert!(
                schema.contains(fragment),
                "schema/schema.sql is missing Project View fragment: {fragment}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn direct_action_upgrade_blocks_active_planned_session_then_removes_old_runtime() {
        let admin_url = isolated_test_database_url();
        let admin = PgPool::connect(&admin_url)
            .await
            .expect("connect database server");
        let database_name = format!(
            "buzz_meeting_direct_upgrade_{}",
            uuid::Uuid::new_v4().simple()
        );
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database_name}"
        )))
        .execute(&admin)
        .await
        .expect("create direct-action migration scratch database");
        let slash = admin_url.rfind('/').expect("database URL has path");
        let database_url = format!("{}/{}", &admin_url[..slash], database_name);
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect direct-action migration scratch database");

        MIGRATOR
            .run_to(45, &pool)
            .await
            .expect("apply migrations through planned Meeting actions");
        let community_id = uuid::Uuid::new_v4();
        let session_id = uuid::Uuid::new_v4();
        let action_run_id = uuid::Uuid::new_v4();
        let host = vec![0x41_u8; 32];
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id)
            .bind(format!("direct-upgrade-{}.test", community_id.simple()))
            .execute(&pool)
            .await
            .expect("seed direct-action migration Community");
        sqlx::query(
            "INSERT INTO channels \
                 (community_id, id, name, channel_type, visibility, created_by, room_kind) \
             VALUES ($1, $2, 'planned-action-upgrade', 'stream', 'private', $3, 'meeting')",
        )
        .bind(community_id)
        .bind(session_id)
        .bind(&host)
        .execute(&pool)
        .await
        .expect("seed planned-action Meeting Channel");
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, schema_version, \
                  floor_policy_version, moderator_pubkey) \
             VALUES ($1, $2, $3, $4, 3, 'moderated-board-actions-v1', $4)",
        )
        .bind(community_id)
        .bind(session_id)
        .bind(vec![0x42_u8; 32])
        .bind(&host)
        .execute(&pool)
        .await
        .expect("seed active planned-action Meeting");
        sqlx::query(
            "INSERT INTO meeting_v2_action_runs \
                 (community_id, session_id, action_run_id, begin_event_id, board_event_id, \
                  control_epoch, board_window, action_phase, action_condition, action_deadline_at) \
             VALUES ($1, $2, $3, $4, $5, 1, 1, 'planning', 'runnable', \
                     clock_timestamp() + interval '5 minutes')",
        )
        .bind(community_id)
        .bind(session_id)
        .bind(action_run_id)
        .bind(vec![0x43_u8; 32])
        .bind(vec![0x44_u8; 32])
        .execute(&pool)
        .await
        .expect("seed active planned-action run");

        let blocked = MIGRATOR.run_to(46, &pool).await;
        assert!(
            blocked
                .as_ref()
                .is_err_and(|error| error.to_string().contains(
                    "cannot remove Meeting planned actions while an active moderated-board-actions-v1 Session exists"
                )),
            "0046 must fail before mutating an active planned-action Meeting: {blocked:?}"
        );
        assert_eq!(applied_versions(&pool).await.last().copied(), Some(45));
        let old_run_survived: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM meeting_v2_action_runs \
             WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3)",
        )
        .bind(community_id)
        .bind(session_id)
        .bind(action_run_id)
        .fetch_one(&pool)
        .await
        .expect("verify failed migration left old run intact");
        assert!(old_run_survived);

        sqlx::query(
            "UPDATE meeting_sessions \
             SET status = 'ended', ended_at = clock_timestamp(), ended_by = $3, \
                 end_event_id = $4, terminal_outcome = 'aborted', \
                 terminal_reason_code = 'migration_fixture' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id)
        .bind(session_id)
        .bind(&host)
        .bind(vec![0x45_u8; 32])
        .execute(&pool)
        .await
        .expect("end planned-action migration fixture");
        MIGRATOR
            .run_to(46, &pool)
            .await
            .expect("replace ended planned-action projections with direct runtime");
        assert_eq!(applied_versions(&pool).await.last().copied(), Some(46));
        let direct_shape: (bool, bool, bool, i64) = sqlx::query_as(
            "SELECT \
                 to_regclass('meeting_v2_action_steps') IS NULL, \
                 to_regclass('meeting_v2_action_step_attempts') IS NULL, \
                 EXISTS (SELECT 1 FROM pg_attribute \
                         WHERE attrelid = 'meeting_v2_action_runs'::regclass \
                           AND attname = 'completion_event_id' AND NOT attisdropped), \
                 (SELECT count(*) FROM meeting_v2_action_runs)",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect direct action runtime after upgrade");
        assert_eq!(direct_shape, (true, true, true, 0));

        // A successful sqlx migrator run retains its session advisory lock on
        // the pooled connection. Reconnect before exercising the next staged
        // deployment exactly as two separate Relay versions would.
        pool.close().await;
        let pool = PgPool::connect(&database_url)
            .await
            .expect("reconnect renewable-action migration scratch database");
        MIGRATOR
            .run_to(47, &pool)
            .await
            .expect("cut ended direct-action history over to renewable leases");
        assert_eq!(applied_versions(&pool).await.last().copied(), Some(47));
        let renewable_shape: (bool, bool, bool) = sqlx::query_as(
            "SELECT \
                 EXISTS (SELECT 1 FROM pg_attribute \
                         WHERE attrelid = 'meeting_v2_action_runs'::regclass \
                           AND attname = 'progress_seq' AND NOT attisdropped), \
                 EXISTS (SELECT 1 FROM pg_attribute \
                         WHERE attrelid = 'meeting_v2_action_runs'::regclass \
                           AND attname = 'operator_hard_deadline' AND NOT attisdropped), \
                 to_regclass('meeting_v2_action_lease_renewals') IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect renewable Action runtime after upgrade");
        assert_eq!(renewable_shape, (true, true, true));

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {database_name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop direct-action migration scratch database");
        admin.close().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn checked_in_schema_builds_project_view_without_a_migration_ledger() {
        let admin_url = isolated_test_database_url();
        let admin = PgPool::connect(&admin_url)
            .await
            .expect("connect database server");
        let database_name = format!("buzz_pv_schema_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database_name}"
        )))
        .execute(&admin)
        .await
        .expect("create schema scratch database");
        let slash = admin_url.rfind('/').expect("database URL has path");
        let database_url = format!("{}/{}", &admin_url[..slash], database_name);
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect schema scratch database");

        sqlx::raw_sql(include_str!("../../../schema/schema.sql"))
            .execute(&pool)
            .await
            .expect("apply checked-in schema");
        let has_ledger: bool =
            sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
                .fetch_one(&pool)
                .await
                .expect("inspect migration ledger");
        assert!(!has_ledger);
        let (project_view_enabled, project_document_enabled, project_context_edge_enabled): (
            bool,
            bool,
            bool,
        ) = sqlx::query_as(
            "INSERT INTO communities (id, host) VALUES ($1, $2) \
             RETURNING project_view_enabled, project_document_enabled, \
                       project_context_edge_enabled",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(format!("schema-{}.test", uuid::Uuid::new_v4().simple()))
        .fetch_one(&pool)
        .await
        .expect("read schema-created Project View default");
        assert!(!project_view_enabled);
        assert!(!project_document_enabled);
        assert!(!project_context_edge_enabled);
        for relation in [
            "project_view_state",
            "project_view_objects",
            "project_view_mutations",
            "project_document_state",
            "project_documents",
            "project_document_revisions",
            "project_document_changes",
            "project_view_context_operations",
            "project_context_edge_state",
            "project_context_edges",
            "project_context_edge_coordinates",
            "project_context_document_bindings",
            "project_context_edge_changes",
        ] {
            let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(format!("public.{relation}"))
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("inspect {relation}: {error}"));
            assert!(exists, "{relation} must exist in checked-in schema");
        }

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {database_name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop schema scratch database");
        admin.close().await;
    }

    #[test]
    fn migration_lint_detects_tables_missing_community_id_by_default() {
        let sql = r#"
            CREATE TABLE communities (id UUID PRIMARY KEY);
            CREATE TABLE widgets (id UUID PRIMARY KEY);
            CREATE TABLE _operator_global_tables (table_name TEXT PRIMARY KEY, reason TEXT NOT NULL);
            INSERT INTO _operator_global_tables (table_name, reason) VALUES
                ('communities', 'tenant registry'),
                ('_operator_global_tables', 'registry');
        "#;

        let definitions = create_table_definitions(sql);
        let scoped = scoped_tables(sql);
        let missing = definitions
            .into_iter()
            .filter(|(table, _)| scoped.contains(table))
            .filter(|(_, definitions)| !table_has_not_null_community_id(definitions))
            .map(|(table, _)| table)
            .collect::<Vec<_>>();

        assert_eq!(missing, vec!["widgets"]);
    }

    #[test]
    fn migration_lint_detects_scoped_key_constraints_not_led_by_community_id() {
        let sql = r#"
            CREATE TABLE widgets (
                community_id UUID NOT NULL,
                id UUID PRIMARY KEY,
                channel_id UUID REFERENCES channels(id),
                slug TEXT,
                CONSTRAINT widgets_name_unique UNIQUE (slug),
                CONSTRAINT widgets_parent_fk FOREIGN KEY (channel_id) REFERENCES channels(id)
            );
            CREATE UNIQUE INDEX idx_widgets_slug ON widgets (slug);
            ALTER TABLE widgets ADD CONSTRAINT widgets_alter_slug_unique UNIQUE (slug);
            ALTER TABLE widgets ADD CONSTRAINT widgets_alter_parent_fk FOREIGN KEY (channel_id) REFERENCES channels(id);
            CREATE TABLE _operator_global_tables (table_name TEXT PRIMARY KEY, reason TEXT NOT NULL);
            INSERT INTO _operator_global_tables (table_name, reason) VALUES
                ('_operator_global_tables', 'registry');
        "#;

        let violations = scoped_constraint_violations(sql);

        assert!(violations
            .iter()
            .any(|violation| violation.kind == ConstraintKind::PrimaryKey));
        assert_eq!(
            violations
                .iter()
                .filter(|violation| violation.kind == ConstraintKind::ForeignKey)
                .count(),
            3
        );
        assert_eq!(
            violations
                .iter()
                .filter(|violation| violation.kind == ConstraintKind::Unique)
                .count(),
            3
        );
    }

    #[test]
    fn migration_lint_accepts_scoped_key_constraints_led_by_community_id() {
        let sql = r#"
            CREATE TABLE widgets (
                community_id UUID NOT NULL,
                id UUID NOT NULL,
                channel_id UUID NOT NULL,
                slug TEXT NOT NULL,
                PRIMARY KEY (community_id, id),
                UNIQUE (community_id, slug),
                FOREIGN KEY (community_id, channel_id) REFERENCES channels(community_id, id)
            );
            CREATE UNIQUE INDEX idx_widgets_slug ON widgets (community_id, slug);
            ALTER TABLE widgets ADD CONSTRAINT widgets_alter_slug_unique UNIQUE (community_id, slug);
            ALTER TABLE widgets ADD CONSTRAINT widgets_alter_parent_fk FOREIGN KEY (community_id, channel_id) REFERENCES channels(community_id, id);
            CREATE TABLE _operator_global_tables (table_name TEXT PRIMARY KEY, reason TEXT NOT NULL);
            INSERT INTO _operator_global_tables (table_name, reason) VALUES
                ('_operator_global_tables', 'registry');
        "#;

        assert!(scoped_constraint_violations(sql).is_empty());
    }

    #[test]
    fn all_non_operator_global_tables_have_not_null_community_id() {
        let sql = migration_sql();
        let sql = sql.as_str();
        let scoped = scoped_tables(sql);
        let missing = create_table_definitions(sql)
            .into_iter()
            .filter(|(table, _)| scoped.contains(table))
            .filter(|(_, definitions)| !table_has_not_null_community_id(definitions))
            .map(|(table, _)| table)
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "every table not listed in _operator_global_tables must carry NOT NULL community_id; missing: {}",
            missing.join(", ")
        );
    }

    #[test]
    fn scoped_primary_key_unique_and_foreign_key_constraints_lead_with_community_id() {
        let sql = migration_sql();
        let sql = sql.as_str();
        let violations = scoped_constraint_violations(sql)
            .into_iter()
            .map(|constraint| {
                format!(
                    "{}. {:?} constraint must lead with community_id: {}",
                    constraint.table, constraint.kind, constraint.description
                )
            })
            .collect::<Vec<_>>();

        assert!(
            violations.is_empty(),
            "tenant-scoped tables are all tables not listed in _operator_global_tables; primary key, unique/FK constraints, and unique indexes on those tables must lead with community_id:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn channels_community_id_is_immutable_after_insert() {
        let sql = migration_sql();
        let sql = sql.as_str();
        let forbidden_mutations = forbidden_channels_community_id_mutations(sql);

        assert!(
            forbidden_mutations.is_empty(),
            "channels.community_id must not be re-tenanted after insert; forbidden migration statements:\n{}",
            forbidden_mutations.join("\n---\n")
        );
        assert!(
            has_channels_community_id_immutability_guard(sql),
            "migrations define channels.community_id but no BEFORE UPDATE trigger/function guard that rejects OLD.community_id <> NEW.community_id was found"
        );
    }

    async fn connect_test_pool() -> PgPool {
        let database_url = isolated_test_database_url();

        PgPool::connect(&database_url)
            .await
            .expect("connect to test DB")
    }

    fn isolated_test_database_url() -> String {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL").expect(
            "destructive migration tests require an explicit BUZZ_TEST_DATABASE_URL; DATABASE_URL is never accepted",
        );
        let database_name = database_name_from_url(&database_url)
            .expect("BUZZ_TEST_DATABASE_URL must include a database name");
        assert!(
            is_disposable_test_database_name(database_name),
            "destructive migration tests require a disposable database whose name starts with buzz_; refused database {database_name}"
        );
        database_url
    }

    fn database_name_from_url(database_url: &str) -> Option<&str> {
        let without_query = database_url
            .split_once('?')
            .map_or(database_url, |(url, _)| url);
        let database_name = without_query.rsplit('/').next()?;
        (!database_name.is_empty()).then_some(database_name)
    }

    fn is_disposable_test_database_name(database_name: &str) -> bool {
        database_name.starts_with("buzz_")
    }

    async fn reset_public_schema(pool: &PgPool) {
        let database_name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await
            .expect("read current test database name before destructive reset");
        assert!(
            is_disposable_test_database_name(&database_name),
            "refusing to reset public schema outside a disposable buzz_ test database: {database_name}"
        );
        sqlx::query("DROP SCHEMA IF EXISTS public CASCADE")
            .execute(pool)
            .await
            .expect("drop public schema");
        sqlx::query("CREATE SCHEMA IF NOT EXISTS public")
            .execute(pool)
            .await
            .expect("create public schema");
    }

    async fn applied_versions(pool: &PgPool) -> Vec<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
        )
        .fetch_all(pool)
        .await
        .expect("read applied migrations")
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn meeting_v2_stage_two_upgrade_preserves_v0_v1_and_stage_one_v2() {
        let pool = connect_test_pool().await;
        reset_public_schema(&pool).await;
        MIGRATOR
            .run_to(42, &pool)
            .await
            .expect("apply migrations through Meeting V1");

        let community_id = uuid::Uuid::new_v4();
        let v0_session = uuid::Uuid::new_v4();
        let v1_session = uuid::Uuid::new_v4();
        let v2_session = uuid::Uuid::new_v4();
        let invalid_v2_session = uuid::Uuid::new_v4();
        let host = vec![0x81_u8; 32];
        let v1_moderator = vec![0x82_u8; 32];
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id)
            .bind(format!("meeting-v2-upgrade-{}.test", community_id.simple()))
            .execute(&pool)
            .await
            .expect("seed pre-V2 community");
        for (session_id, name) in [
            (v0_session, "V0 preserved"),
            (v1_session, "V1 preserved"),
            (v2_session, "V2 valid"),
            (invalid_v2_session, "V2 invalid"),
        ] {
            sqlx::query(
                "INSERT INTO channels \
                     (community_id, id, name, channel_type, visibility, created_by, room_kind) \
                 VALUES ($1, $2, $3, 'stream', 'private', $4, 'meeting')",
            )
            .bind(community_id)
            .bind(session_id)
            .bind(name)
            .bind(&host)
            .execute(&pool)
            .await
            .expect("seed Meeting Channel");
        }
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, \
                  schema_version, floor_policy_version) \
             VALUES ($1, $2, $3, $4, 1, 'uniform-v0')",
        )
        .bind(community_id)
        .bind(v0_session)
        .bind([0x83_u8; 32].as_slice())
        .bind(&host)
        .execute(&pool)
        .await
        .expect("seed V0 Session before V2 upgrade");
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, \
                  schema_version, floor_policy_version, moderator_pubkey) \
             VALUES ($1, $2, $3, $4, 2, 'moderated-baton-v1', $5)",
        )
        .bind(community_id)
        .bind(v1_session)
        .bind([0x84_u8; 32].as_slice())
        .bind(&host)
        .bind(&v1_moderator)
        .execute(&pool)
        .await
        .expect("seed V1 Session before V2 upgrade");
        sqlx::query(
            "INSERT INTO meeting_participants \
                 (community_id, session_id, pubkey, participant_type, channel_role) \
             VALUES ($1, $2, $3, 'human', 'member')",
        )
        .bind(community_id)
        .bind(v1_session)
        .bind(&v1_moderator)
        .execute(&pool)
        .await
        .expect("seed frozen V1 participant");

        MIGRATOR
            .run_to(43, &pool)
            .await
            .expect("upgrade Meeting schema through V2 stage one");
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, \
                  schema_version, floor_policy_version, moderator_pubkey) \
             VALUES ($1, $2, $3, $4, 3, 'moderated-board-v1', $4)",
        )
        .bind(community_id)
        .bind(v2_session)
        .bind([0x85_u8; 32].as_slice())
        .bind(&host)
        .execute(&pool)
        .await
        .expect("seed stage-one V2 Session");
        sqlx::query(
            "INSERT INTO meeting_v2_bootstrap_state (community_id, session_id) \
             VALUES ($1, $2)",
        )
        .bind(community_id)
        .bind(v2_session)
        .execute(&pool)
        .await
        .expect("seed stage-one V2 bootstrap runtime");
        sqlx::query(
            "INSERT INTO meeting_current_boards \
                 (community_id, session_id, board_event_id, board_format, board_content) \
             VALUES ($1, $2, $3, 'markdown', '# Existing stage-one Board')",
        )
        .bind(community_id)
        .bind(v2_session)
        .bind([0x87_u8; 32].as_slice())
        .execute(&pool)
        .await
        .expect("seed stage-one V2 current Board");

        run_migrations(&pool)
            .await
            .expect("upgrade Meeting schema through V2 stage two");
        assert_eq!(applied_versions(&pool).await.last().copied(), Some(51));
        let preserved: Vec<(uuid::Uuid, i32, String, Option<Vec<u8>>)> = sqlx::query_as(
            "SELECT session_id, schema_version, floor_policy_version, moderator_pubkey \
             FROM meeting_sessions WHERE community_id = $1 ORDER BY session_id",
        )
        .bind(community_id)
        .fetch_all(&pool)
        .await
        .expect("read preserved V0/V1/V2 Sessions");
        assert_eq!(preserved.len(), 3);
        assert!(preserved.iter().any(|row| {
            row.0 == v0_session && row.1 == 1 && row.2 == "uniform-v0" && row.3.is_none()
        }));
        assert!(preserved.iter().any(|row| {
            row.0 == v1_session
                && row.1 == 2
                && row.2 == "moderated-baton-v1"
                && row.3.as_deref() == Some(v1_moderator.as_slice())
        }));
        assert!(preserved.iter().any(|row| {
            row.0 == v2_session
                && row.1 == 3
                && row.2 == "moderated-board-v1"
                && row.3.as_deref() == Some(host.as_slice())
        }));
        let new_projection_rows: (i64, i64, i64) = sqlx::query_as(
            "SELECT \
                 (SELECT count(*) FROM meeting_current_boards), \
                 (SELECT count(*) FROM meeting_v2_bootstrap_state), \
                 (SELECT count(*) FROM meeting_v2_config)",
        )
        .fetch_one(&pool)
        .await
        .expect("count additive V2 projections");
        assert_eq!(new_projection_rows, (1, 1, 0));
        let upgraded_runtime: (String, i64, i64, Option<chrono::DateTime<chrono::Utc>>) =
            sqlx::query_as(
                "SELECT runtime_phase, control_epoch, board_window, board_deadline_at \
                 FROM meeting_v2_bootstrap_state \
                 WHERE community_id = $1 AND session_id = $2",
            )
            .bind(community_id)
            .bind(v2_session)
            .fetch_one(&pool)
            .await
            .expect("read upgraded stage-one V2 runtime");
        assert_eq!(upgraded_runtime, ("bootstrap_locked".into(), 1, 0, None));
        let invalid = sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, \
                  schema_version, floor_policy_version, moderator_pubkey) \
             VALUES ($1, $2, $3, $4, 3, 'moderated-board-v1', $5)",
        )
        .bind(community_id)
        .bind(invalid_v2_session)
        .bind([0x86_u8; 32].as_slice())
        .bind(&host)
        .bind(&v1_moderator)
        .execute(&pool)
        .await;
        assert!(invalid.is_err(), "V2 moderator must equal the creator");
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn project_view_upgrade_from_0024_is_additive_and_disabled() {
        let admin_url = isolated_test_database_url();
        let admin = PgPool::connect(&admin_url)
            .await
            .expect("connect database server");
        let database_name = format!("buzz_pv_upgrade_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database_name}"
        )))
        .execute(&admin)
        .await
        .expect("create migration scratch database");
        let slash = admin_url.rfind('/').expect("database URL has path");
        let database_url = format!("{}/{}", &admin_url[..slash], database_name);
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect migration scratch database");

        MIGRATOR
            .run_to(24, &pool)
            .await
            .expect("apply migrations through 0024");
        let active_id = uuid::Uuid::new_v4();
        let archived_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2), ($3, $4)")
            .bind(active_id)
            .bind(format!("active-{}.test", active_id.simple()))
            .bind(archived_id)
            .bind(format!("archived-{}.test", archived_id.simple()))
            .execute(&pool)
            .await
            .expect("seed pre-0025 communities");
        sqlx::query("UPDATE communities SET archived_at = now() WHERE id = $1")
            .bind(archived_id)
            .execute(&pool)
            .await
            .expect("archive pre-0025 community");

        run_migrations(&pool)
            .await
            .expect("upgrade scratch database through 0051");
        assert_eq!(applied_versions(&pool).await.last().copied(), Some(51));
        let flags: Vec<(uuid::Uuid, bool, i16)> = sqlx::query_as(
            "SELECT id, project_view_enabled, project_view_schema_version \
             FROM communities ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .expect("read upgraded feature flags");
        assert_eq!(flags.len(), 2);
        assert!(flags
            .iter()
            .all(|(_, enabled, schema)| !enabled && *schema == 1));
        let legacy_rows: Vec<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT id, host FROM communities ORDER BY id")
                .fetch_all(&pool)
                .await
                .expect("pre-feature community projection remains readable");
        assert_eq!(legacy_rows.len(), 2);
        let greenfield_id = uuid::Uuid::new_v4();
        let greenfield: (bool, i16) = sqlx::query_as(
            "INSERT INTO communities (id, host) VALUES ($1, $2) \
             RETURNING project_view_enabled, project_view_schema_version",
        )
        .bind(greenfield_id)
        .bind(format!("greenfield-{}.test", greenfield_id.simple()))
        .fetch_one(&pool)
        .await
        .expect("read greenfield Project View defaults");
        assert_eq!(greenfield, (false, 3));
        let maintenance: (String, Option<i64>) = sqlx::query_as(
            "SELECT state, current_epoch FROM project_view_maintenance \
             WHERE community_id = $1",
        )
        .bind(greenfield_id)
        .fetch_one(&pool)
        .await
        .expect("read greenfield Project View maintenance seed");
        assert_eq!(maintenance, ("normal".to_owned(), None));
        let has_state: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM project_view_state WHERE community_id = $1)",
        )
        .bind(greenfield_id)
        .fetch_one(&pool)
        .await
        .expect("inspect greenfield Project View state");
        assert!(!has_state);

        let greenfield_owner = "11".repeat(32);
        let greenfield_member = "22".repeat(32);
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role, added_by) \
             VALUES ($1, $2, 'owner', NULL), ($1, $3, 'member', $2)",
        )
        .bind(greenfield_id)
        .bind(&greenfield_owner)
        .bind(&greenfield_member)
        .execute(&pool)
        .await
        .expect("retain ordinary membership before Project View initialization");

        // Archiving is Community lifecycle state, not Project View canonical
        // state. It must remain possible before initialization.
        sqlx::query("UPDATE communities SET archived_at = now() WHERE id = $1")
            .bind(greenfield_id)
            .execute(&pool)
            .await
            .expect("archive uninitialized schema-v3 Community");
        sqlx::query("UPDATE communities SET archived_at = NULL WHERE id = $1")
            .bind(greenfield_id)
            .execute(&pool)
            .await
            .expect("unarchive uninitialized schema-v3 Community");

        for (schema_version, view_enabled, context_enabled, label) in [
            (2_i16, false, false, "schema-v2 missing state"),
            (3_i16, true, false, "enabled schema-v3 missing state"),
            (3_i16, true, true, "context-enabled schema-v3 missing state"),
        ] {
            let invalid_id = uuid::Uuid::new_v4();
            let invalid = sqlx::query(
                "INSERT INTO communities \
                    (id, host, project_view_schema_version, project_view_enabled, \
                     project_context_enabled) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(invalid_id)
            .bind(format!(
                "invalid-{}-{}.test",
                label.replace(' ', "-"),
                invalid_id.simple()
            ))
            .bind(schema_version)
            .bind(view_enabled)
            .bind(context_enabled)
            .execute(&pool)
            .await;
            assert!(invalid.is_err(), "{label} must fail closed");
        }
        let legacy_event_id = vec![0x25_u8; 32];
        let legacy_actor = vec![0x26_u8; 32];
        let legacy_meta_id = vec![0x27_u8; 32];
        let legacy_signer = vec![0x28_u8; 32];
        let mut legacy_tx = pool
            .begin()
            .await
            .expect("begin legacy v1 write-shape transaction");
        let legacy_state: (i16, Vec<u8>, Option<Vec<u8>>) = sqlx::query_as(
            "INSERT INTO project_view_state ( \
                 community_id, project_revision, active_object_count, \
                 initialized_at, updated_at, last_event_id, last_actor_pubkey, \
                 meta_projection_event_id, projection_pubkey, projection_generation \
             ) VALUES ($1, 1, 0, now(), now(), $2, $3, $4, $5, 1) \
             RETURNING schema_version, last_change_id, last_source_event_id",
        )
        .bind(active_id)
        .bind(&legacy_event_id)
        .bind(&legacy_actor)
        .bind(&legacy_meta_id)
        .bind(&legacy_signer)
        .fetch_one(&mut *legacy_tx)
        .await
        .expect("migration 0027 accepts the exact v1 state insert shape");
        assert_eq!(
            legacy_state,
            (1, legacy_event_id.clone(), Some(legacy_event_id))
        );

        let next_legacy_event_id = vec![0x29_u8; 32];
        let mirrored_change_id: Vec<u8> = sqlx::query_scalar(
            "UPDATE project_view_state \
             SET project_revision = 2, updated_at = now(), last_event_id = $2 \
             WHERE community_id = $1 \
             RETURNING last_change_id",
        )
        .bind(active_id)
        .bind(&next_legacy_event_id)
        .fetch_one(&mut *legacy_tx)
        .await
        .expect("migration 0027 accepts the exact v1 state update shape");
        assert_eq!(mirrored_change_id, next_legacy_event_id);
        legacy_tx
            .rollback()
            .await
            .expect("rollback isolated v1 write-shape probe");

        for table in [
            "project_view_state",
            "project_view_objects",
            "project_view_mutations",
            "project_view_changes",
            "project_role_assignments",
            "project_role_assignment_proposals",
            "project_work_commitments",
            "project_role_checkpoints",
            "project_role_handoffs",
            "project_role_continuity_references",
        ] {
            let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(format!("public.{table}"))
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("inspect {table}: {error}"));
            assert!(exists, "{table} must exist after upgrade");
        }

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {database_name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop migration scratch database");
        admin.close().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn project_document_upgrade_from_0031_is_additive_and_disabled() {
        let admin_url = isolated_test_database_url();
        let admin = PgPool::connect(&admin_url)
            .await
            .expect("connect database server");
        let database_name = format!("buzz_pd_upgrade_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database_name}"
        )))
        .execute(&admin)
        .await
        .expect("create migration scratch database");
        let slash = admin_url.rfind('/').expect("database URL has path");
        let database_url = format!("{}/{}", &admin_url[..slash], database_name);
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect migration scratch database");

        MIGRATOR
            .run_to(31, &pool)
            .await
            .expect("apply migrations through 0031");
        let existing_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(existing_id)
            .bind(format!("document-upgrade-{}.test", existing_id.simple()))
            .execute(&pool)
            .await
            .expect("seed pre-0032 Community");

        run_migrations(&pool)
            .await
            .expect("upgrade scratch database through 0051");
        assert_eq!(applied_versions(&pool).await.last().copied(), Some(51));
        let existing_enabled: bool =
            sqlx::query_scalar("SELECT project_document_enabled FROM communities WHERE id = $1")
                .bind(existing_id)
                .fetch_one(&pool)
                .await
                .expect("read upgraded Community flag");
        assert!(!existing_enabled);
        let new_enabled: bool = sqlx::query_scalar(
            "INSERT INTO communities (id, host) VALUES ($1, $2) \
             RETURNING project_document_enabled",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(format!(
            "document-new-{}.test",
            uuid::Uuid::new_v4().simple()
        ))
        .fetch_one(&pool)
        .await
        .expect("read new Community default");
        assert!(!new_enabled);
        for relation in [
            "project_document_state",
            "project_documents",
            "project_document_revisions",
            "project_document_changes",
            "idx_project_documents_active",
            "idx_project_document_revisions_history",
        ] {
            let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(format!("public.{relation}"))
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("inspect {relation}: {error}"));
            assert!(exists, "{relation} must exist after migration 0032");
        }

        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {database_name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop migration scratch database");
        admin.close().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn concurrent_migrators_reach_project_view_schema_once() {
        let admin_url = isolated_test_database_url();
        let admin = PgPool::connect(&admin_url)
            .await
            .expect("connect database server");
        let database_name = format!(
            "buzz_pv_concurrent_migrate_{}",
            uuid::Uuid::new_v4().simple()
        );
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database_name}"
        )))
        .execute(&admin)
        .await
        .expect("create concurrent-migration scratch database");
        let slash = admin_url.rfind('/').expect("database URL has path");
        let database_url = format!("{}/{}", &admin_url[..slash], database_name);
        let first = PgPool::connect(&database_url)
            .await
            .expect("connect first migrator");
        let second = PgPool::connect(&database_url)
            .await
            .expect("connect second migrator");

        let (first_result, second_result) =
            tokio::join!(run_migrations(&first), run_migrations(&second));
        first_result.expect("first concurrent migrator succeeds");
        second_result.expect("second concurrent migrator succeeds");
        assert_eq!(applied_versions(&first).await.last().copied(), Some(51));
        let project_view_migration_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM _sqlx_migrations \
             WHERE version BETWEEN 25 AND 36 AND success",
        )
        .fetch_one(&first)
        .await
        .expect("count Project View migration ledger entries");
        assert_eq!(project_view_migration_count, 12);

        first.close().await;
        second.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {database_name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop concurrent-migration scratch database");
        admin.close().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn pre_0007_ambiguous_nip_rs_data_blocks_without_mutation_and_allows_retry() {
        let pool = connect_test_pool().await;
        reset_public_schema(&pool).await;
        MIGRATOR
            .run_to(6, &pool)
            .await
            .expect("apply migrations 1-6");

        let community_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id)
            .bind(format!("pre-0007-{}.example", community_id.simple()))
            .execute(&pool)
            .await
            .expect("insert community");
        let event_id = vec![1_u8; 32];
        let pubkey = vec![2_u8; 32];
        let d_tag = format!("read-state:{}", "a".repeat(32));
        let ambiguous_tags = serde_json::json!([["d", d_tag], ["d", "other"], ["t", "read-state"]]);
        sqlx::query(
            "INSERT INTO events \
             (community_id, id, pubkey, created_at, kind, tags, content, sig, received_at, d_tag) \
             VALUES ($1, $2, $3, NOW(), 30078, $4, 'ambiguous', $5, NOW(), $6)",
        )
        .bind(community_id)
        .bind(&event_id)
        .bind(&pubkey)
        .bind(&ambiguous_tags)
        .bind(vec![3_u8; 64])
        .bind(&d_tag)
        .execute(&pool)
        .await
        .expect("insert ambiguous NIP-RS row");

        let before_versions = applied_versions(&pool).await;
        let before_row: (serde_json::Value, String) =
            sqlx::query_as("SELECT tags, content FROM events WHERE community_id=$1 AND id=$2")
                .bind(community_id)
                .bind(&event_id)
                .fetch_one(&pool)
                .await
                .expect("read ambiguous row before blocked migration");
        let blocked = run_migrations(&pool).await;
        assert!(blocked.is_err(), "ambiguous pre-0007 data must fail closed");
        assert_eq!(applied_versions(&pool).await, before_versions);
        let after_row: (serde_json::Value, String) =
            sqlx::query_as("SELECT tags, content FROM events WHERE community_id=$1 AND id=$2")
                .bind(community_id)
                .bind(&event_id)
                .fetch_one(&pool)
                .await
                .expect("blocked migration must preserve source row");
        assert_eq!(after_row, before_row);

        let repaired_tags = serde_json::json!([["d", d_tag], ["t", "read-state"]]);
        sqlx::query("UPDATE events SET tags=$1 WHERE community_id=$2 AND id=$3")
            .bind(repaired_tags)
            .bind(community_id)
            .bind(&event_id)
            .execute(&pool)
            .await
            .expect("repair ambiguous row");
        run_migrations(&pool)
            .await
            .expect("retry succeeds after operator repair");
        assert_eq!(applied_versions(&pool).await.last().copied(), Some(51));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn populated_upgrade_preserves_search_policy_except_for_push_leases() {
        let pool = connect_test_pool().await;
        reset_public_schema(&pool).await;
        MIGRATOR
            .run_to(7, &pool)
            .await
            .expect("apply migrations 1-7");

        let community_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id)
            .bind(format!("pre-0008-{}.example", community_id.simple()))
            .execute(&pool)
            .await
            .expect("insert community");

        for (marker, kind) in [(1_u8, 1_i32), (2_u8, 30_350_i32)] {
            sqlx::query(
                "INSERT INTO events \
                 (community_id, id, pubkey, created_at, kind, tags, content, sig, received_at) \
                 VALUES ($1, $2, $3, NOW(), $4, '[]'::jsonb, 'brownfield needle', $5, NOW())",
            )
            .bind(community_id)
            .bind(vec![marker; 32])
            .bind(vec![marker + 10; 32])
            .bind(kind)
            .bind(vec![marker + 20; 64])
            .execute(&pool)
            .await
            .expect("insert brownfield event");
        }

        MIGRATOR
            .run_to(11, &pool)
            .await
            .expect("apply main migrations through 11");
        let before: Vec<(i32, bool)> = sqlx::query_as(
            "SELECT kind, search_tsv @@ plainto_tsquery('simple', 'needle') \
             FROM events ORDER BY kind",
        )
        .fetch_all(&pool)
        .await
        .expect("read pre-push search behavior");
        assert_eq!(before, vec![(1, true), (30_350, true)]);

        run_migrations(&pool)
            .await
            .expect("apply push migrations to populated database");
        let after: Vec<(i32, Option<bool>)> = sqlx::query_as(
            "SELECT kind, search_tsv @@ plainto_tsquery('simple', 'needle') \
             FROM events ORDER BY kind",
        )
        .fetch_all(&pool)
        .await
        .expect("read post-push search behavior");
        assert_eq!(after, vec![(1, Some(true)), (30_350, None)]);
    }

    #[test]
    fn destructive_migration_tests_only_accept_disposable_database_names() {
        assert!(is_disposable_test_database_name("buzz_migration_test"));
        assert!(is_disposable_test_database_name(
            "buzz_meeting_contract_123"
        ));
        assert!(!is_disposable_test_database_name("buzz"));
        assert!(!is_disposable_test_database_name("postgres"));
        assert!(!is_disposable_test_database_name("production"));

        assert_eq!(
            database_name_from_url("postgres://user:secret@localhost:5432/buzz_migration_test"),
            Some("buzz_migration_test")
        );
        assert_eq!(
            database_name_from_url(
                "postgres://user:secret@localhost:5432/buzz_migration_test?sslmode=disable"
            ),
            Some("buzz_migration_test")
        );
    }
}
