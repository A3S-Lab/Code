//! For a token that is not a JWT, `parse_jwt_expiration` returns `None` and `is_jwt_expired_or_near` returns `false`.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
struct Claims {
    exp: Option<i64>,
    sub: Option<String>,
}

pub fn parse_jwt_expiration(token: &str) -> Option<DateTime<Utc>> {
    jsonwebtoken::dangerous::insecure_decode::<Claims>(token)
        .ok()
        .and_then(|data| data.claims.exp)
        .and_then(|ts| DateTime::from_timestamp(ts, 0))
}

/// The `sub` claim, the account id the issuer knows the user by. `None` for a token that is not a JWT or has no subject
pub fn parse_jwt_subject(token: &str) -> Option<String> {
    jsonwebtoken::dangerous::insecure_decode::<Claims>(token)
        .ok()
        .and_then(|data| data.claims.sub)
        .filter(|sub| !sub.is_empty())
}

pub fn is_jwt_expired_or_near(token: &str, threshold: Duration) -> bool {
    parse_jwt_expiration(token)
        .map(|exp| exp <= Utc::now() + threshold)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jwt_subject_reads_the_sub_claim() {
        let token = build_test_jwt(r#"{"sub":"auth0|user_1","exp":1772575524}"#);
        assert_eq!(Some("auth0|user_1".to_owned()), parse_jwt_subject(&token));
        assert_eq!(None, parse_jwt_subject(&build_test_jwt(r#"{"exp":1}"#)));
    }

    /// Tokens with an `aud` claim must parse successfully.
    /// `jsonwebtoken::Validation::default()` enables audience validation which silently rejects these tokens unless `validate_aud = false` is set.
    #[test]
    fn parses_jwt_with_aud_claim() {
        let token = build_test_jwt(r#"{"aud":["some-audience"],"exp":1772575524}"#);
        let exp = parse_jwt_expiration(&token);
        assert_eq!(exp.unwrap().timestamp(), 1772575524);
    }

    fn build_test_jwt(payload_json: &str) -> String {
        use base64::Engine;
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = enc.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = enc.encode(payload_json);
        format!("{header}.{payload}.fake-signature")
    }
}
