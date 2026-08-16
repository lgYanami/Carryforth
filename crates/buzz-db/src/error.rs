//! Database error types.

use thiserror::Error;

/// Errors produced by database operations.
#[derive(Debug, Error)]
pub enum DbError {
    /// A SQLx driver-level error.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// A SQLx migration error.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// Attempted to store an AUTH event (kind 22242), which is forbidden.
    #[error("AUTH events (kind 22242) must not be stored")]
    AuthEventRejected,

    /// Attempted to store an ephemeral event (kinds 20000–29999), which is forbidden.
    #[error("ephemeral events (kind {0}) must not be stored")]
    EphemeralEventRejected(u16),

    /// The requested channel does not exist.
    #[error("channel not found: {0}")]
    ChannelNotFound(uuid::Uuid),

    /// The requested member is not in the channel.
    #[error("member not found in channel {0}")]
    MemberNotFound(uuid::Uuid),

    /// A generic not-found error.
    #[error("not found: {0}")]
    NotFound(String),

    /// The caller lacks permission for the requested operation.
    #[error("access denied: {0}")]
    AccessDenied(String),

    /// JSON serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A value in the database is malformed or unexpected.
    #[error("invalid data: {0}")]
    InvalidData(String),

    /// A stored timestamp value could not be interpreted.
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(i64),
}

/// Convenience alias for `Result<T, DbError>`.
pub type Result<T> = std::result::Result<T, DbError>;

/// Effect phase of a semantic-scoped database operation.
///
/// Classification is decided by where the operation observed the failure,
/// never by re-parsing public error text. Only the read-snapshot and
/// release-confirmation phases accept classified transients; every other
/// phase treats an unlisted error as terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticDbEffectPhase {
    /// Reading the authorized semantic query ticket.
    TicketRead,
    /// Committing a Provider slot reservation.
    ProviderReservation,
    /// Confirming final Provider egress authorization.
    EgressConfirmation,
    /// Reading inside an open repeatable-read snapshot.
    SnapshotRead,
    /// Closing a repeatable-read snapshot.
    SnapshotClose,
    /// Confirming a result release.
    ReleaseConfirmation,
}

impl SemanticDbEffectPhase {
    /// Closed low-cardinality metric label for this phase.
    pub const fn metric_label(self) -> &'static str {
        match self {
            Self::TicketRead => "ticket_read",
            Self::ProviderReservation => "provider_reservation",
            Self::EgressConfirmation => "egress_confirmation",
            Self::SnapshotRead => "snapshot_read",
            Self::SnapshotClose => "snapshot_close",
            Self::ReleaseConfirmation => "release_confirmation",
        }
    }

    /// True when this phase accepts classified transient SQLSTATE errors.
    const fn accepts_classified_transients(self) -> bool {
        matches!(
            self,
            Self::SnapshotRead | Self::SnapshotClose | Self::ReleaseConfirmation
        )
    }
}

/// Closed SQLSTATE classes accepted as classified semantic transients.
///
/// This is the frozen allowlist required by the unified reliability runtime
/// plan: any `Sqlx` error outside these classes is terminal by default and
/// is never retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticDbSqlstateClass {
    /// `40001 serialization_failure` and `40P01 deadlock_detected`.
    TransactionRollback,
    /// `55P03 lock_not_available`.
    LockUnavailable,
    /// `57014 query_canceled`, `57P01 admin_shutdown`, `57P02 crash_shutdown`.
    OperatorIntervention,
    /// SQLSTATE class `08` connection exceptions.
    ConnectionException,
    /// `53300 too_many_connections`.
    ConnectionLimit,
}

impl SemanticDbSqlstateClass {
    /// Map a PostgreSQL SQLSTATE to its closed class, if allowlisted.
    pub fn from_sqlstate(code: &str) -> Option<Self> {
        match code {
            "40001" | "40P01" => Some(Self::TransactionRollback),
            "55P03" => Some(Self::LockUnavailable),
            "57014" | "57P01" | "57P02" => Some(Self::OperatorIntervention),
            "53300" => Some(Self::ConnectionLimit),
            other if other.len() == 5 && other.starts_with("08") => Some(Self::ConnectionException),
            _ => None,
        }
    }

    /// Closed low-cardinality metric label for this class.
    pub const fn metric_label(self) -> &'static str {
        match self {
            Self::TransactionRollback => "transaction_rollback",
            Self::LockUnavailable => "lock_unavailable",
            Self::OperatorIntervention => "operator_intervention",
            Self::ConnectionException => "connection_exception",
            Self::ConnectionLimit => "connection_limit",
        }
    }
}

/// Classified database failure for interactive semantic operations.
///
/// Outcome-unknown results (a reservation commit or release confirmation
/// whose outcome cannot be determined) are deliberately absent: they cannot
/// be inferred from an error value and must be reported by the call site
/// that owns the transaction boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticDbFailureKind {
    /// The database denied authorization for this request.
    AuthorizationDenied,
    /// A stored value or invariant was violated.
    InvariantViolation,
    /// A read-snapshot transient from the closed SQLSTATE allowlist.
    SnapshotReadTransient {
        /// Closed SQLSTATE class of the transient.
        sqlstate_class: SemanticDbSqlstateClass,
    },
    /// A release-confirmation transient from the closed SQLSTATE allowlist.
    ReleaseConfirmationTransient {
        /// Closed SQLSTATE class of the transient.
        sqlstate_class: SemanticDbSqlstateClass,
    },
    /// An unlisted failure; terminal by default and never retried.
    UnclassifiedTerminal,
}

impl SemanticDbFailureKind {
    /// Classify a raw SQLSTATE for one effect phase.
    ///
    /// Only phases that accept classified transients map an allowlisted
    /// SQLSTATE; everything else stays terminal.
    pub fn from_sqlstate(code: &str, phase: SemanticDbEffectPhase) -> Self {
        if !phase.accepts_classified_transients() {
            return Self::UnclassifiedTerminal;
        }
        match (SemanticDbSqlstateClass::from_sqlstate(code), phase) {
            (Some(sqlstate_class), SemanticDbEffectPhase::ReleaseConfirmation) => {
                Self::ReleaseConfirmationTransient { sqlstate_class }
            }
            (Some(sqlstate_class), _) => Self::SnapshotReadTransient { sqlstate_class },
            (None, _) => Self::UnclassifiedTerminal,
        }
    }
}

impl DbError {
    /// Classify this error for an interactive semantic operation phase.
    ///
    /// `AccessDenied` currently conflates authorization, generation, and
    /// readiness denials; until the owning call sites report those outcomes
    /// directly, every such denial classifies as [`SemanticDbFailureKind::AuthorizationDenied`].
    pub fn semantic_failure_kind(&self, phase: SemanticDbEffectPhase) -> SemanticDbFailureKind {
        match self {
            Self::AccessDenied(_) => SemanticDbFailureKind::AuthorizationDenied,
            Self::InvalidData(_) => SemanticDbFailureKind::InvariantViolation,
            Self::Sqlx(error) => match error.as_database_error().and_then(|db| db.code()) {
                Some(code) => SemanticDbFailureKind::from_sqlstate(&code, phase),
                None => SemanticDbFailureKind::UnclassifiedTerminal,
            },
            _ => SemanticDbFailureKind::UnclassifiedTerminal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlstate_allowlist_is_closed() {
        assert_eq!(
            SemanticDbSqlstateClass::from_sqlstate("40001"),
            Some(SemanticDbSqlstateClass::TransactionRollback)
        );
        assert_eq!(
            SemanticDbSqlstateClass::from_sqlstate("40P01"),
            Some(SemanticDbSqlstateClass::TransactionRollback)
        );
        assert_eq!(
            SemanticDbSqlstateClass::from_sqlstate("55P03"),
            Some(SemanticDbSqlstateClass::LockUnavailable)
        );
        assert_eq!(
            SemanticDbSqlstateClass::from_sqlstate("57014"),
            Some(SemanticDbSqlstateClass::OperatorIntervention)
        );
        assert_eq!(
            SemanticDbSqlstateClass::from_sqlstate("08006"),
            Some(SemanticDbSqlstateClass::ConnectionException)
        );
        assert_eq!(
            SemanticDbSqlstateClass::from_sqlstate("53300"),
            Some(SemanticDbSqlstateClass::ConnectionLimit)
        );
        assert_eq!(SemanticDbSqlstateClass::from_sqlstate("23505"), None);
        assert_eq!(SemanticDbSqlstateClass::from_sqlstate(""), None);
        assert_eq!(SemanticDbSqlstateClass::from_sqlstate("42P01"), None);
    }

    #[test]
    fn classified_transients_apply_only_to_snapshot_and_release_phases() {
        assert_eq!(
            SemanticDbFailureKind::from_sqlstate("40001", SemanticDbEffectPhase::SnapshotRead),
            SemanticDbFailureKind::SnapshotReadTransient {
                sqlstate_class: SemanticDbSqlstateClass::TransactionRollback
            }
        );
        assert_eq!(
            SemanticDbFailureKind::from_sqlstate("57014", SemanticDbEffectPhase::SnapshotClose),
            SemanticDbFailureKind::SnapshotReadTransient {
                sqlstate_class: SemanticDbSqlstateClass::OperatorIntervention
            }
        );
        assert_eq!(
            SemanticDbFailureKind::from_sqlstate(
                "08001",
                SemanticDbEffectPhase::ReleaseConfirmation
            ),
            SemanticDbFailureKind::ReleaseConfirmationTransient {
                sqlstate_class: SemanticDbSqlstateClass::ConnectionException
            }
        );
        // Ticket, reservation, and egress phases never classify transients.
        for phase in [
            SemanticDbEffectPhase::TicketRead,
            SemanticDbEffectPhase::ProviderReservation,
            SemanticDbEffectPhase::EgressConfirmation,
        ] {
            assert_eq!(
                SemanticDbFailureKind::from_sqlstate("40001", phase),
                SemanticDbFailureKind::UnclassifiedTerminal
            );
        }
    }

    #[test]
    fn db_error_classifies_denial_and_invariant_without_sqlstate() {
        assert_eq!(
            DbError::AccessDenied("restricted".to_owned())
                .semantic_failure_kind(SemanticDbEffectPhase::SnapshotRead),
            SemanticDbFailureKind::AuthorizationDenied
        );
        assert_eq!(
            DbError::InvalidData("malformed".to_owned())
                .semantic_failure_kind(SemanticDbEffectPhase::ReleaseConfirmation),
            SemanticDbFailureKind::InvariantViolation
        );
        assert_eq!(
            DbError::Serde(serde_json::from_str::<()>("not-json").unwrap_err())
                .semantic_failure_kind(SemanticDbEffectPhase::SnapshotRead),
            SemanticDbFailureKind::UnclassifiedTerminal
        );
    }
}
