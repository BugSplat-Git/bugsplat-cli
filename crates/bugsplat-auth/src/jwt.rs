//! Reads the signed-in email out of an access token, for display.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// The email an access token names, if it names one. The signature is not checked, so
/// this is for `whoami` to show and never for a decision.
pub(crate) fn email(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    // BugSplat's access tokens come from Cognito, which puts the email in ID tokens only;
    // `username` is the email only for accounts whose Cognito username is their email.
    ["email", "username"]
        .iter()
        .filter_map(|claim| claims.get(claim)?.as_str())
        .find(|value| value.contains('@'))
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) fn fake_token(claims: &serde_json::Value) -> String {
    let encode = |value: &serde_json::Value| URL_SAFE_NO_PAD.encode(value.to_string());
    format!(
        "{}.{}.signature",
        encode(&serde_json::json!({"alg": "none"})),
        encode(claims)
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_an_email_claim() {
        let token = fake_token(&json!({"email": "fred@example.com", "username": "abc"}));
        assert_eq!(email(&token).as_deref(), Some("fred@example.com"));
    }

    #[test]
    fn reads_a_username_that_is_an_email() {
        let token = fake_token(&json!({"username": "fred@example.com"}));
        assert_eq!(email(&token).as_deref(), Some("fred@example.com"));
    }

    #[test]
    fn a_username_that_is_not_an_email_is_not_one() {
        let token = fake_token(&json!({"username": "15e9142e-dd17-4a1a-b7c1-536d55288e7a"}));
        assert_eq!(email(&token), None);
    }

    #[test]
    fn a_token_that_is_not_a_jwt_has_no_email() {
        assert_eq!(email("opaque-token"), None);
        assert_eq!(email("a.b.c"), None);
        assert_eq!(email(""), None);
    }
}
