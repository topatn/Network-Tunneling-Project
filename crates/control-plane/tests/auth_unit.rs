//! Unit tests for authentication, PKCE S256 exchange, session rotation, and token verification
//! (`T-040`, `SEC-001`, `SEC-003`, `FR-001`, `ADR-018`, `contracts.md` §§5.1–5.4).

use chrono::{Duration, Utc};
use gtt_control_plane::routes::auth::{
    create_access_token, generate_refresh_token, verify_access_token, REFRESH_TOKEN_PREFIX,
};
use gtt_protocol::http::{ApiErrorCode, AuthExchangeRequest, UserProfileResponse};
use gtt_protocol::validate::{
    compute_pkce_s256_challenge, is_valid_pkce_verifier, is_valid_redirect_uri,
};
use uuid::Uuid;

#[test]
fn pkce_verifier_validation_and_s256_challenge_computation() -> Result<(), String> {
    // 1. Valid RFC 7636 verifier (43-128 chars, unreserved characters)
    let valid_verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    if !is_valid_pkce_verifier(valid_verifier) {
        return Err("expected valid verifier to pass".to_owned());
    }

    // RFC 7636 Appendix B test vector
    let challenge = compute_pkce_s256_challenge(valid_verifier);
    if challenge != "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM" {
        return Err(format!("unexpected challenge: {challenge}"));
    }

    // 2. Too short (< 43) or too long (> 128)
    if is_valid_pkce_verifier("too-short-verifier") {
        return Err("expected short verifier to fail".to_owned());
    }
    let too_long = "a".repeat(129);
    if is_valid_pkce_verifier(&too_long) {
        return Err("expected long verifier to fail".to_owned());
    }

    // 3. Prohibited characters (e.g. spaces, symbols not in -._~)
    let invalid_chars = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk+bad=";
    if is_valid_pkce_verifier(invalid_chars) {
        return Err("expected invalid chars to fail".to_owned());
    }
    Ok(())
}

#[test]
fn plain_pkce_method_is_prohibited_per_adr018() -> Result<(), String> {
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let redirect = "http://127.0.0.1:5173/auth/callback";

    // Request declaring "plain" PKCE method MUST be rejected with validation_failed
    let plain_req = AuthExchangeRequest {
        code: "test_code_123".to_owned(),
        code_verifier: verifier.to_owned(),
        redirect_uri: redirect.to_owned(),
        device_label: None,
        code_challenge_method: Some("plain".to_owned()),
    };
    match plain_req.validate() {
        Err(e) => {
            if !e.to_string().contains("plain PKCE method is prohibited") {
                return Err(format!("unexpected error message: {e}"));
            }
        }
        Ok(()) => return Err("expected plain PKCE method to be rejected".to_owned()),
    }

    // Request declaring "S256" is accepted
    let s256_req = AuthExchangeRequest {
        code: "test_code_123".to_owned(),
        code_verifier: verifier.to_owned(),
        redirect_uri: redirect.to_owned(),
        device_label: None,
        code_challenge_method: Some("S256".to_owned()),
    };
    s256_req.validate().map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn redirect_uri_validation_rules() -> Result<(), String> {
    if !is_valid_redirect_uri("http://127.0.0.1:5173/auth/callback") {
        return Err("expected 127.0.0.1 callback to pass".to_owned());
    }
    if !is_valid_redirect_uri("http://localhost:8080/callback") {
        return Err("expected localhost callback to pass".to_owned());
    }
    if !is_valid_redirect_uri("gtt://auth/callback") {
        return Err("expected gtt scheme callback to pass".to_owned());
    }

    // Missing scheme
    if is_valid_redirect_uri("localhost:5173/auth") {
        return Err("expected missing scheme to fail".to_owned());
    }
    // Empty or containing whitespace
    if is_valid_redirect_uri("") {
        return Err("expected empty URI to fail".to_owned());
    }
    if is_valid_redirect_uri("http://127.0.0.1:5173 /auth") {
        return Err("expected whitespace in URI to fail".to_owned());
    }
    Ok(())
}

#[test]
fn refresh_token_generation_entropy_and_prefix() -> Result<(), String> {
    let token1 = generate_refresh_token();
    let token2 = generate_refresh_token();

    if !token1.starts_with(REFRESH_TOKEN_PREFIX) || !token2.starts_with(REFRESH_TOKEN_PREFIX) {
        return Err("expected token to start with prefix".to_owned());
    }
    if token1 == token2 {
        return Err("expected two generated tokens to differ".to_owned());
    }
    if token1.len() < 48 {
        return Err(format!("expected token length >= 48, got {}", token1.len()));
    }
    Ok(())
}

#[test]
fn access_token_issuance_verification_and_tampering_rejection() -> Result<(), String> {
    let signing_key = [0x77u8; 32];
    let user_id = Uuid::new_v4();
    let now = Utc::now();

    // 1. Issue access token
    let token = create_access_token(user_id, &signing_key, 900, now)
        .map_err(|e| format!("failed to issue token: {e:?}"))?;
    let verified_id = verify_access_token(&token, &signing_key, now)
        .map_err(|e| format!("failed to verify token: {e:?}"))?;
    if verified_id != user_id {
        return Err("user_id mismatch".to_owned());
    }

    // 2. Tampered token signature fails verification
    let mut tampered = token.clone();
    tampered.push('x');
    if verify_access_token(&tampered, &signing_key, now).is_ok() {
        return Err("expected tampered token to fail verification".to_owned());
    }

    // 3. Expired token fails verification
    let future = now + Duration::seconds(901);
    match verify_access_token(&token, &signing_key, future) {
        Err(expired_err) => {
            if expired_err.response.error.code != ApiErrorCode::Unauthenticated {
                return Err(format!(
                    "unexpected error code: {:?}",
                    expired_err.response.error.code
                ));
            }
            if !expired_err.response.error.message.contains("expired") {
                return Err(format!(
                    "unexpected error message: {}",
                    expired_err.response.error.message
                ));
            }
        }
        Ok(_) => return Err("expected expired token to fail verification".to_owned()),
    }

    // 4. Token signed with different key fails verification
    let wrong_key = [0x88u8; 32];
    if verify_access_token(&token, &wrong_key, now).is_ok() {
        return Err("expected wrong key to fail verification".to_owned());
    }

    Ok(())
}

#[test]
fn user_profile_response_validation() -> Result<(), String> {
    let valid = UserProfileResponse {
        id: Uuid::new_v4().to_string(),
        display_name: Some("TestHost".to_owned()),
        status: "active".to_owned(),
    };
    valid.validate().map_err(|e| e.to_string())?;

    let invalid_id = UserProfileResponse {
        id: "not-a-uuid".to_owned(),
        display_name: None,
        status: "active".to_owned(),
    };
    if invalid_id.validate().is_ok() {
        return Err("expected invalid id to fail validation".to_owned());
    }

    let invalid_status = UserProfileResponse {
        id: Uuid::new_v4().to_string(),
        display_name: None,
        status: "unknown_status".to_owned(),
    };
    if invalid_status.validate().is_ok() {
        return Err("expected invalid status to fail validation".to_owned());
    }

    Ok(())
}
