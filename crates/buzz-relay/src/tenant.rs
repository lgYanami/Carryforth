//! Row-zero host binding: resolve the request's community from the connection
//! host *before* any handler observes tenant data.
//!
//! Conformance "row zero": `req.community = resolve_host(connection.host)`,
//! bound at connection establishment. The host is the authoritative selector;
//! an unknown or unmapped host fails closed with a generic rejection and never
//! falls through to a default tenant. A client-supplied community (e.g. a token
//! stamp or an `h` tag) may narrow or authenticate authority but can never
//! override the host-derived community.
//!
//! This module owns the *seam* (the [`HostResolver`] trait and the fail-closed
//! [`bind_community`] helper) and the relay-side call site. The DB-backed
//! implementation that queries the `communities` table lives in `buzz-db`
//! (`Db::resolve_host`); the relay depends on the trait, not the query, so the
//! binding is testable without a database.

use buzz_core::tenant::{normalize_host, CommunityId, TenantContext};

/// Resolves a normalized connection host to its community, or `None` when the
/// host maps to no community on this deployment.
///
/// Implementors MUST treat the input as already normalized by
/// [`buzz_core::tenant::normalize_host`] — [`bind_community`] guarantees that,
/// so the stored `communities.host` key and the lookup key agree by
/// construction (the column is `UNIQUE(lower(host))`, frozen in migration
/// `0001`).
///
/// Uses a native `async fn` in trait (no `async-trait` dependency). The relay
/// holds a concrete resolver (`Db`), so callers are generic over `R:
/// HostResolver` and never need `dyn` dispatch.
pub trait HostResolver: Send + Sync {
    /// The error type surfaced when the lookup itself fails (e.g. the database
    /// is unreachable). This is distinct from "host not mapped", which is a
    /// successful lookup returning `None`.
    type Error;

    /// Look up the community for an already-normalized host.
    ///
    /// `Ok(Some(_))` — host maps to a community.
    /// `Ok(None)` — host is valid input but maps to nothing (fail closed).
    /// `Err(_)` — the lookup could not be performed.
    fn resolve_host(
        &self,
        normalized_host: &str,
    ) -> impl std::future::Future<Output = Result<Option<CommunityId>, Self::Error>> + Send;
}

/// The outcome of attempting to bind a request to a community.
#[derive(Debug)]
pub enum BindError<E> {
    /// The host did not map to any community on this deployment. Callers MUST
    /// reject the request with a *generic* error — never echo the host back or
    /// distinguish "unmapped" from other failures, so an unauthenticated
    /// caller cannot probe which hosts exist.
    UnmappedHost,
    /// The resolution lookup itself failed (e.g. database error). Treated as
    /// fail-closed: the request is rejected, never admitted to a default tenant.
    Lookup(E),
}

/// Closed public-surface classification for a Postgres-backed host lookup.
///
/// This classification deliberately distinguishes a successful lookup that
/// found no tenant from a lookup that could not be completed. It never carries
/// the requested host, SQL text, tenant identity, or the underlying error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostLookupFailureClass {
    /// The database answered successfully and the normalized host is unmapped.
    Unmapped,
    /// A transient database or pool condition prevented a trustworthy answer.
    DependencyUnavailable(HostLookupDependencyReason),
    /// A schema, decode, configuration, or otherwise unclassified failure.
    Internal(HostLookupInternalReason),
}

/// Low-cardinality reasons for retryable host-lookup dependency failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostLookupDependencyReason {
    /// The dedicated control pool could not provide a connection in time.
    PoolAcquireTimeout,
    /// The dedicated control pool is closing or its worker terminated.
    PoolUnavailable,
    /// The connection failed at the transport or TLS layer.
    ConnectionUnavailable,
    /// PostgreSQL is starting, recovering, or shutting down.
    DatabaseRecovery,
    /// PostgreSQL reported insufficient resources.
    ResourceExhaustion,
}

impl HostLookupDependencyReason {
    /// Stable metric/log code. Contains no request or database content.
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::PoolAcquireTimeout => "pool_acquire_timeout",
            Self::PoolUnavailable => "pool_unavailable",
            Self::ConnectionUnavailable => "connection_unavailable",
            Self::DatabaseRecovery => "database_recovery",
            Self::ResourceExhaustion => "resource_exhaustion",
        }
    }
}

/// Low-cardinality reasons for non-retryable host-lookup failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostLookupInternalReason {
    /// The query or schema contract does not match the running database.
    SchemaContract,
    /// A returned value could not be decoded into the frozen host-map shape.
    DecodeContract,
    /// The failure did not match the closed dependency set.
    UnknownInternal,
}

impl HostLookupInternalReason {
    /// Stable metric/log code. Contains no request or database content.
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::SchemaContract => "schema_contract",
            Self::DecodeContract => "decode_contract",
            Self::UnknownInternal => "unknown_internal",
        }
    }
}

/// Classify one database-backed row-zero failure without leaking its contents.
pub(crate) fn classify_db_bind_error(
    error: &BindError<buzz_db::DbError>,
) -> HostLookupFailureClass {
    match error {
        BindError::UnmappedHost => HostLookupFailureClass::Unmapped,
        BindError::Lookup(buzz_db::DbError::Sqlx(error)) => classify_sqlx_host_lookup_error(error),
        BindError::Lookup(
            buzz_db::DbError::InvalidData(_)
            | buzz_db::DbError::InvalidTimestamp(_)
            | buzz_db::DbError::Serde(_),
        ) => HostLookupFailureClass::Internal(HostLookupInternalReason::DecodeContract),
        BindError::Lookup(buzz_db::DbError::Migrate(_)) => {
            HostLookupFailureClass::Internal(HostLookupInternalReason::SchemaContract)
        }
        BindError::Lookup(_) => {
            HostLookupFailureClass::Internal(HostLookupInternalReason::UnknownInternal)
        }
    }
}

fn classify_sqlx_host_lookup_error(error: &sqlx::Error) -> HostLookupFailureClass {
    use sqlx::Error;

    match error {
        Error::PoolTimedOut => HostLookupFailureClass::DependencyUnavailable(
            HostLookupDependencyReason::PoolAcquireTimeout,
        ),
        Error::PoolClosed | Error::WorkerCrashed => HostLookupFailureClass::DependencyUnavailable(
            HostLookupDependencyReason::PoolUnavailable,
        ),
        Error::Io(_) | Error::Tls(_) | Error::BeginFailed => {
            HostLookupFailureClass::DependencyUnavailable(
                HostLookupDependencyReason::ConnectionUnavailable,
            )
        }
        Error::Database(database) => classify_sqlstate(database.code().as_deref()),
        Error::ColumnDecode { .. } | Error::Decode(_) | Error::Protocol(_) => {
            HostLookupFailureClass::Internal(HostLookupInternalReason::DecodeContract)
        }
        Error::TypeNotFound { .. }
        | Error::ColumnIndexOutOfBounds { .. }
        | Error::ColumnNotFound(_)
        | Error::Configuration(_)
        | Error::InvalidArgument(_)
        | Error::Encode(_)
        | Error::AnyDriverError(_)
        | Error::RowNotFound
        | Error::InvalidSavePointStatement
        | Error::ConfigFile(_) => {
            HostLookupFailureClass::Internal(HostLookupInternalReason::SchemaContract)
        }
        Error::Migrate(_) => {
            HostLookupFailureClass::Internal(HostLookupInternalReason::SchemaContract)
        }
        _ => HostLookupFailureClass::Internal(HostLookupInternalReason::UnknownInternal),
    }
}

fn classify_sqlstate(code: Option<&str>) -> HostLookupFailureClass {
    match code {
        Some(code) if code.starts_with("08") => HostLookupFailureClass::DependencyUnavailable(
            HostLookupDependencyReason::ConnectionUnavailable,
        ),
        Some(code) if code.starts_with("53") => HostLookupFailureClass::DependencyUnavailable(
            HostLookupDependencyReason::ResourceExhaustion,
        ),
        Some("57P01" | "57P02" | "57P03") => HostLookupFailureClass::DependencyUnavailable(
            HostLookupDependencyReason::DatabaseRecovery,
        ),
        Some(code) if code.starts_with("42") => {
            HostLookupFailureClass::Internal(HostLookupInternalReason::SchemaContract)
        }
        _ => HostLookupFailureClass::Internal(HostLookupInternalReason::UnknownInternal),
    }
}

/// Record one row-zero failure without including high-cardinality request data.
pub(crate) fn record_host_lookup_failure(class: HostLookupFailureClass) {
    let (class_code, reason) = match class {
        HostLookupFailureClass::Unmapped => ("unmapped", "unmapped"),
        HostLookupFailureClass::DependencyUnavailable(reason) => {
            ("dependency_unavailable", reason.code())
        }
        HostLookupFailureClass::Internal(reason) => ("internal", reason.code()),
    };
    metrics::counter!(
        "buzz_row_zero_host_lookup_failures_total",
        "class" => class_code,
        "reason" => reason
    )
    .increment(1);
}

/// Content-free HTTP projection for a row-zero host lookup failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HostLookupHttpFailure {
    pub(crate) status: axum::http::StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    pub(crate) retryable: bool,
}

/// Convert a database-backed row-zero failure to the stable public HTTP shape.
pub(crate) fn host_lookup_http_failure(
    error: &BindError<buzz_db::DbError>,
) -> HostLookupHttpFailure {
    let class = classify_db_bind_error(error);
    record_host_lookup_failure(class);
    match class {
        HostLookupFailureClass::Unmapped => HostLookupHttpFailure {
            status: axum::http::StatusCode::NOT_FOUND,
            code: "not_found:relay:community_lookup",
            message: "relay: no community is configured for this host",
            retryable: false,
        },
        HostLookupFailureClass::DependencyUnavailable(_) => HostLookupHttpFailure {
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            code: "unavailable:relay:community_lookup",
            message: "relay: community lookup is temporarily unavailable",
            retryable: true,
        },
        HostLookupFailureClass::Internal(_) => HostLookupHttpFailure {
            status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal:relay:community_lookup",
            message: "relay: community lookup failed",
            retryable: false,
        },
    }
}

/// Bind a raw connection host to a [`TenantContext`], failing closed.
///
/// This is the single row-zero entry point. It normalizes the host with the
/// one shared rule, resolves it, and on any non-success (unmapped *or* lookup
/// error) returns a [`BindError`] the caller turns into a generic rejection.
/// There is deliberately no path that yields a default or fallback community.
///
/// The returned [`TenantContext`] carries the *normalized* host, so downstream
/// NIP-05 / audit labelling and the NIP-98 `u`-host check all see the same
/// canonical form the community was resolved from.
pub async fn bind_community<R: HostResolver>(
    resolver: &R,
    raw_host: &str,
) -> Result<TenantContext, BindError<R::Error>> {
    let host = normalize_host(raw_host);
    // Inv_RowZero (host-binding seam): an empty raw_host carries no community
    // evidence — there is no `connection.host` to resolve, so no community can
    // be derived from it. Fail closed BEFORE the resolver lookup. The schema
    // does not forbid an `host = ''` row in `communities`, so without this
    // guard a request with a missing/whitespace-only Host header would silently
    // bind to a misconfigured empty-host community. Reuse `UnmappedHost` (not a
    // distinct variant) so the rejection is byte-identical to any other unmapped
    // host — an unauthenticated caller cannot probe for an empty-host row.
    if host.is_empty() {
        return Err(BindError::UnmappedHost);
    }
    match resolver.resolve_host(&host).await {
        Ok(Some(community)) => Ok(TenantContext::resolved(community, host)),
        Ok(None) => Err(BindError::UnmappedHost),
        Err(e) => Err(BindError::Lookup(e)),
    }
}

/// Resolve the deployment's own community from the configured relay URL host.
///
/// For server-internal paths that have no inbound request `Host` header — the
/// git Smart-HTTP transport, the localhost pre-receive hook callback, the
/// workflow execution sink, and startup tasks — the tenant cannot come from a
/// connection. A relay deployment serves a single canonical host (its
/// `relay_url`), so we resolve that host through the same fail-closed
/// [`bind_community`] path. This is deliberately NOT a default/fallback
/// community: an unmapped `relay_url` host returns the same [`BindError`] as
/// any other unmapped host.
pub async fn bind_deployment_community<R: HostResolver>(
    resolver: &R,
    relay_url: &str,
) -> Result<TenantContext, BindError<R::Error>> {
    bind_community(resolver, &buzz_core::tenant::relay_url_authority(relay_url)).await
}

/// Extract the relay URL authority in the same normalized shape as request
/// `Host` headers and `communities.host`: host plus an explicit non-default
/// port, if present.
///
/// `pub` so startup ([`crate::main`], a separate binary crate) can seed the
/// deployment's own community under the *same* normalized host that live request
/// resolution ([`bind_community`]) will derive — the two must agree or the
/// bootstrapped owner lands in a community no request ever resolves to.
///
/// This is a thin re-export of [`buzz_core::tenant::relay_url_authority`]: the
/// canonical implementation lives in `buzz-core` so the relay seam *and* the
/// `buzz-admin` CLI derive a byte-identical authority (same port/IPv6 handling).
pub use buzz_core::tenant::relay_url_authority;

/// Production [`HostResolver`]: the relay resolves hosts against the durable
/// `communities` host map in Postgres.
///
/// This is the *only* place the relay couples the row-zero seam to buzz-db. The
/// trait keeps `bind_community` and every call site database-free and testable;
/// this impl is the thin adapter from buzz-db's `lookup_community_by_host`
/// (which returns a `CommunityRecord`) to the seam's `CommunityId`. A lookup
/// that succeeds but finds no row is `Ok(None)` — fail-closed, never a default
/// tenant; a lookup that *fails* (DB unreachable) is `Err`, also fail-closed.
impl HostResolver for buzz_db::Db {
    type Error = buzz_db::DbError;

    async fn resolve_host(
        &self,
        normalized_host: &str,
    ) -> Result<Option<CommunityId>, Self::Error> {
        Ok(self
            .lookup_community_by_host(normalized_host)
            .await?
            .map(|record| record.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn sqlstate_classification_is_fail_closed_and_narrow() {
        assert_eq!(
            classify_sqlstate(Some("08006")),
            HostLookupFailureClass::DependencyUnavailable(
                HostLookupDependencyReason::ConnectionUnavailable
            )
        );
        assert_eq!(
            classify_sqlstate(Some("57P03")),
            HostLookupFailureClass::DependencyUnavailable(
                HostLookupDependencyReason::DatabaseRecovery
            )
        );
        assert_eq!(
            classify_sqlstate(Some("53300")),
            HostLookupFailureClass::DependencyUnavailable(
                HostLookupDependencyReason::ResourceExhaustion
            )
        );
        assert_eq!(
            classify_sqlstate(Some("42P01")),
            HostLookupFailureClass::Internal(HostLookupInternalReason::SchemaContract)
        );
        assert_eq!(
            classify_sqlstate(Some("23505")),
            HostLookupFailureClass::Internal(HostLookupInternalReason::UnknownInternal)
        );
        assert_eq!(
            classify_sqlstate(None),
            HostLookupFailureClass::Internal(HostLookupInternalReason::UnknownInternal)
        );
    }

    #[test]
    fn sqlx_pool_and_decode_errors_have_distinct_public_classes() {
        assert_eq!(
            classify_sqlx_host_lookup_error(&sqlx::Error::PoolTimedOut),
            HostLookupFailureClass::DependencyUnavailable(
                HostLookupDependencyReason::PoolAcquireTimeout
            )
        );
        assert_eq!(
            classify_sqlx_host_lookup_error(&sqlx::Error::PoolClosed),
            HostLookupFailureClass::DependencyUnavailable(
                HostLookupDependencyReason::PoolUnavailable
            )
        );
        assert_eq!(
            classify_sqlx_host_lookup_error(&sqlx::Error::ColumnNotFound("host".to_owned())),
            HostLookupFailureClass::Internal(HostLookupInternalReason::SchemaContract)
        );
    }

    #[test]
    fn public_host_lookup_failures_distinguish_unmapped_transient_and_internal() {
        let unmapped = host_lookup_http_failure(&BindError::UnmappedHost);
        assert_eq!(unmapped.status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(unmapped.code, "not_found:relay:community_lookup");
        assert!(!unmapped.retryable);

        let transient = host_lookup_http_failure(&BindError::Lookup(buzz_db::DbError::Sqlx(
            sqlx::Error::PoolTimedOut,
        )));
        assert_eq!(
            transient.status,
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(transient.code, "unavailable:relay:community_lookup");
        assert!(transient.retryable);

        let internal = host_lookup_http_failure(&BindError::Lookup(buzz_db::DbError::InvalidData(
            "sensitive database detail".to_owned(),
        )));
        assert_eq!(
            internal.status,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(internal.code, "internal:relay:community_lookup");
        assert!(!internal.retryable);
        assert!(!internal.message.contains("sensitive"));
    }

    /// In-memory resolver over a fixed host→community map, so the binding seam
    /// is testable without a database.
    struct MapResolver {
        map: HashMap<String, CommunityId>,
        fail: bool,
    }

    impl HostResolver for MapResolver {
        type Error = &'static str;
        async fn resolve_host(
            &self,
            normalized_host: &str,
        ) -> Result<Option<CommunityId>, Self::Error> {
            if self.fail {
                return Err("db down");
            }
            Ok(self.map.get(normalized_host).copied())
        }
    }

    fn resolver_with(host: &str, id: u128) -> MapResolver {
        let mut map = HashMap::new();
        map.insert(
            host.to_string(),
            CommunityId::from_uuid(Uuid::from_u128(id)),
        );
        MapResolver { map, fail: false }
    }

    #[tokio::test]
    async fn maps_known_host_to_its_community() {
        let r = resolver_with("relay.example", 1);
        let ctx = bind_community(&r, "relay.example").await.expect("bound");
        assert_eq!(ctx.community().as_uuid(), &Uuid::from_u128(1));
        assert_eq!(ctx.host(), "relay.example");
    }

    #[tokio::test]
    async fn normalizes_before_lookup_so_variants_resolve_to_one_tenant() {
        // The map holds the canonical form; case/dot/default-port variants must
        // all bind to the same community (they cannot split a tenant).
        let r = resolver_with("relay.example", 7);
        for variant in ["RELAY.EXAMPLE", "relay.example.", "relay.example:443"] {
            let ctx = bind_community(&r, variant)
                .await
                .unwrap_or_else(|_| panic!("variant {variant:?} should bind"));
            assert_eq!(
                ctx.community().as_uuid(),
                &Uuid::from_u128(7),
                "variant {variant:?}"
            );
            assert_eq!(ctx.host(), "relay.example", "variant {variant:?}");
        }
    }

    #[tokio::test]
    async fn deployment_url_keeps_nondefault_port_for_lookup() {
        let r = resolver_with("localhost:3000", 42);
        let ctx = bind_deployment_community(&r, "ws://localhost:3000")
            .await
            .expect("deployment host should bind with non-default port");
        assert_eq!(ctx.community().as_uuid(), &Uuid::from_u128(42));
        assert_eq!(ctx.host(), "localhost:3000");

        let wrong = resolver_with("localhost", 42);
        let err = bind_deployment_community(&wrong, "ws://localhost:3000")
            .await
            .unwrap_err();
        assert!(matches!(err, BindError::UnmappedHost));
    }

    #[tokio::test]
    async fn deployment_url_normalizes_default_ports() {
        let r = resolver_with("relay.example", 9);
        for url in ["ws://relay.example:80", "wss://relay.example:443"] {
            let ctx = bind_deployment_community(&r, url)
                .await
                .unwrap_or_else(|_| panic!("url {url:?} should bind"));
            assert_eq!(ctx.community().as_uuid(), &Uuid::from_u128(9));
            assert_eq!(ctx.host(), "relay.example", "url {url:?}");
        }
    }

    #[test]
    fn relay_url_authority_preserves_ipv6_brackets() {
        assert_eq!(relay_url_authority("ws://[::1]:3000"), "[::1]:3000");
        assert_eq!(relay_url_authority("wss://[::1]:443"), "[::1]");
    }

    #[tokio::test]
    async fn unmapped_host_fails_closed() {
        let r = resolver_with("relay.example", 1);
        let err = bind_community(&r, "evil.example").await.unwrap_err();
        assert!(matches!(err, BindError::UnmappedHost));
    }

    #[tokio::test]
    async fn lookup_error_fails_closed_not_default_tenant() {
        let r = MapResolver {
            map: HashMap::new(),
            fail: true,
        };
        let err = bind_community(&r, "relay.example").await.unwrap_err();
        assert!(matches!(err, BindError::Lookup("db down")));
    }

    mod redteam_attack2 {
        use super::*;

        /// RED gate. Configures a resolver with an `""→CommunityId` mapping
        /// (the schema permits it; no CHECK against empty host exists), then
        /// asks `bind_community` to bind an empty raw_host as a request with
        /// a missing/invalid Host header would. Today this returns
        /// `Ok(TenantContext{community=X})` — the fence collapses to the
        /// misconfigured row. The fix: short-circuit in `bind_community` so
        /// that `normalize_host(raw_host).is_empty()` returns
        /// `Err(BindError::UnmappedHost)` before any resolver lookup.
        ///
        /// Generic-rejection note: we reuse `UnmappedHost` (not a new
        /// `EmptyHost` variant) so the door's response is byte-identical to
        /// any other unmapped host — an unauthenticated caller cannot probe
        /// whether the deployment has an empty-host row.
        ///
        /// Delete this `#[ignore]` when the fix lands; verified RED with
        /// `cargo test -p buzz-relay --include-ignored
        ///   tenant::tests::redteam_attack2::empty_raw_host_fails_closed_even_if_db_has_empty_host_row`

        #[tokio::test]
        async fn empty_raw_host_fails_closed_even_if_db_has_empty_host_row() {
            // Simulate operator misconfig / buggy migration: an empty-host row
            // exists in `communities`. The schema does not forbid this.
            let r = resolver_with("", 0xdeadbeef);

            // A request with a missing or unreadable Host header reaches
            // `bind_community` with raw_host = "" (router.rs:169-172). The
            // fence must reject — the request never supplied a host.
            let err = bind_community(&r, "").await.expect_err(
                "Inv_RowZero: an empty raw_host carries no community evidence; \
                 bind_community must fail closed regardless of the host map",
            );
            assert!(
                matches!(err, BindError::UnmappedHost),
                "fence must produce a generic UnmappedHost (no info leak about \
                 whether an empty-host row exists); got {err:?}",
            );
        }

        /// RED gate. Same property, whitespace-only host: `normalize_host`
        /// trims to empty (`buzz-core::tenant::normalize_host_empty_stays_empty`),
        /// so this is the same fence collapse via a different raw input.
        ///
        /// Delete `#[ignore]` when the fix lands.

        #[tokio::test]
        async fn whitespace_only_raw_host_fails_closed_even_if_db_has_empty_host_row() {
            let r = resolver_with("", 0xdeadbeef);

            let err = bind_community(&r, "   ").await.expect_err(
                "Inv_RowZero: whitespace-only raw_host normalizes to empty \
                 (see buzz-core::tenant::normalize_host) and carries no \
                 community evidence",
            );
            assert!(
                matches!(err, BindError::UnmappedHost),
                "fence must produce a generic UnmappedHost; got {err:?}",
            );
        }

        /// Negative control: a *non-empty* unmapped host must still fail
        /// closed (this already passes — included so the redteam_attack2
        /// module documents both shapes of the fence's intended behavior and
        /// catches a fix that accidentally over-narrows to only-empty).
        #[tokio::test]
        async fn non_empty_unmapped_host_still_fails_closed_after_fix() {
            let r = resolver_with("", 0xdeadbeef);
            let err = bind_community(&r, "evil.example").await.unwrap_err();
            assert!(matches!(err, BindError::UnmappedHost));
        }
    }
}
