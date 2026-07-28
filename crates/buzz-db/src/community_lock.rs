//! Shared Community/Project transaction lock.
//!
//! Project View state and Community membership form one consistency boundary
//! once a Community moves to Project View v2. Keeping the lock primitive in a
//! neutral module prevents the Project View writer, NIP-43 snapshot publisher,
//! and membership writers from silently drifting to different lock keys or
//! acquisition orders.

use buzz_core::CommunityId;
use sqlx::{Postgres, Transaction};

const COMMUNITY_PROJECT_LOCK_NAMESPACE: &str = "buzz_project_view:";

/// Acquire the transaction-scoped Community/Project advisory lock.
///
/// Writers use the exclusive form. Snapshot-only readers may use the shared
/// form when they need a revision-consistent inspection without mutation.
pub(crate) async fn acquire(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    shared: bool,
) -> Result<(), sqlx::Error> {
    let function = if shared {
        "pg_advisory_xact_lock_shared"
    } else {
        "pg_advisory_xact_lock"
    };
    let sql = format!("SELECT {function}(hashtextextended($1, 0))");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(format!(
            "{COMMUNITY_PROJECT_LOCK_NAMESPACE}{}",
            community_id.as_uuid()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
