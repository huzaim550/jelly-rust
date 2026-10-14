//! Bearer-token authentication.
//!
//! Jellyfin clients send the access token as `X-Emby-Token` (or
//! `X-MediaBrowser-Token`), and browsers send it as an `api_key` query
//! parameter because `<img>` and `<video>` cannot set headers. Both are
//! accepted, and both go through the same lookup.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use jf_core::dto::UserRecord;

use crate::state::AppState;

/// A request authenticated as a specific user.
#[derive(Clone)]
pub struct AuthUser {
    pub user: UserRecord,
    pub session_id: String,
    pub token: String,
}

impl AuthUser {
    pub fn id(&self) -> &str {
        &self.user.id
    }

    pub fn is_admin(&self) -> bool {
        self.user.policy.is_administrator
    }
}

/// Returned when a route needs a valid token and does not have one.
#[derive(Debug)]
pub struct Unauthorized(pub String);

impl IntoResponse for Unauthorized {
    fn into_response(self) -> Response {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "Message": self.0, "Status": 401 })),
        )
            .into_response()
    }
}

/// Extract the raw token from headers or the query string.
pub fn extract_token(parts: &Parts) -> Option<String> {
    let headers = &parts.headers;

    for name in ["x-emby-token", "x-mediabrowser-token", "authorization"] {
        if let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) {
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            // Authorization may carry a Bearer prefix.
            let token = value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
                .unwrap_or(value);
            return Some(token.to_owned());
        }
    }

    // `<video src>` and `<img src>` cannot set headers.
    let query = parts.uri.query()?;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key.eq_ignore_ascii_case("api_key") || key.eq_ignore_ascii_case("ApiKey") {
                return Some(percent_decode(value));
            }
        }
    }

    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Require a valid access token on a route.
///
/// The `AsRef<AppState>` bound is what lets this work with
/// `axum::extract::State<AppState>` without any unsafe casting: axum's `State`
/// implements `AsRef<T>`, and the state is only reachable through that.
pub struct RequireAuth(pub AuthUser);

impl<S> FromRequestParts<S> for RequireAuth
where
    S: AsRef<AppState> + Send + Sync,
{
    type Rejection = Unauthorized;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app: &AppState = state.as_ref();

        let token = extract_token(parts)
            .ok_or_else(|| Unauthorized("This request requires an authentication token.".into()))?;

        let (session_id, user) = app
            .db
            .session_by_token(&token)
            .await
            .map_err(|e| Unauthorized(format!("Authentication failed: {e}")))?
            .ok_or_else(|| {
                Unauthorized("Invalid authentication token. Please sign in again.".into())
            })?;

        // A disabled account must stop working immediately, not at next login.
        if user.policy.is_disabled {
            return Err(Unauthorized("This account has been disabled.".into()));
        }

        let _ = app.db.touch_session(&session_id).await;
        let _ = app.db.touch_activity(&user.id).await;

        Ok(RequireAuth(AuthUser { user, session_id, token }))
    }
}

/// Optional authentication: resolves the user when a token is present.
pub struct MaybeAuth(pub Option<AuthUser>);

impl<S> FromRequestParts<S> for MaybeAuth
where
    S: AsRef<AppState> + Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app: &AppState = state.as_ref();

        let Some(token) = extract_token(parts) else {
            return Ok(MaybeAuth(None));
        };

        let resolved = app.db.session_by_token(&token).await.ok().flatten();
        let Some((session_id, user)) = resolved else {
            return Ok(MaybeAuth(None));
        };

        if user.policy.is_disabled {
            return Ok(MaybeAuth(None));
        }

        Ok(MaybeAuth(Some(AuthUser { user, session_id, token })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("abc"), "abc");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("a+b"), "a b");
        // A malformed escape is passed through rather than panicking.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}