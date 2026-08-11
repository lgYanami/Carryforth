use buzz_semantic::Digest32;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{QueryContractResult, SemanticGraphQueryError};

const HTTP_REQUEST_BINDING_DOMAIN: &[u8] = b"buzz-semantic-http-request\0";

/// Derive the authenticated HTTP request binding used by a signed query
/// result. The body is hashed exactly as received, before parsing.
pub fn derive_http_request_binding(
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> QueryContractResult<Digest32> {
    if host_project_id.is_nil() || host_project_id.get_version_num() != 4 {
        return Err(SemanticGraphQueryError::InvalidUuid {
            field: "host_project_id",
        });
    }
    let body_digest: [u8; 32] = Sha256::digest(exact_authenticated_body).into();
    let mut hasher = Sha256::new();
    hasher.update(HTTP_REQUEST_BINDING_DOMAIN);
    hasher.update(host_project_id.as_bytes());
    hasher.update(authenticated_caller_pubkey);
    hasher.update(nip98_auth_event_id.as_bytes());
    hasher.update(body_digest);
    Ok(Digest32::from_bytes(hasher.finalize().into()))
}

/// Verify a result binding against the authenticated HTTP transcript.
pub fn verify_http_request_binding(
    observed: Digest32,
    host_project_id: Uuid,
    authenticated_caller_pubkey: &[u8; 32],
    nip98_auth_event_id: Digest32,
    exact_authenticated_body: &[u8],
) -> QueryContractResult<()> {
    let expected = derive_http_request_binding(
        host_project_id,
        authenticated_caller_pubkey,
        nip98_auth_event_id,
        exact_authenticated_body,
    )?;
    if observed != expected {
        return Err(SemanticGraphQueryError::InvalidState(
            "HTTP request binding digest mismatch".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use buzz_semantic::Digest32;
    use uuid::Uuid;

    use super::{derive_http_request_binding, verify_http_request_binding};

    #[test]
    fn exact_http_binding_has_a_fixed_golden_and_binds_every_body_byte() {
        let project_id =
            Uuid::parse_str("123e4567-e89b-42d3-a456-426614174000").expect("UUID fixture");
        let caller = [0x11; 32];
        let auth_event = Digest32::from_bytes([0x22; 32]);
        let body = br#"{"request_id":"123e4567-e89b-42d3-a456-426614174001","problem":"why?"}"#;
        let binding =
            derive_http_request_binding(project_id, &caller, auth_event, body).expect("binding");
        assert_eq!(
            binding.to_hex(),
            "fd74d6eb1846a5bcf49eb9ac0ecf0fa0a16bfed18dcf0ae87fcf3946693aba32"
        );
        assert!(
            verify_http_request_binding(binding, project_id, &caller, auth_event, body,).is_ok()
        );
        assert_ne!(
            binding,
            derive_http_request_binding(
                project_id,
                &caller,
                auth_event,
                br#"{"request_id":"123e4567-e89b-42d3-a456-426614174001","problem":"why?","budget":{}}"#,
            )
            .expect("changed body binding")
        );
    }
}
