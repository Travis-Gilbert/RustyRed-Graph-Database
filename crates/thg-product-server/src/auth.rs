use axum::http::{HeaderMap, StatusCode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthContext {
    pub token: String,
    pub scopes: Vec<String>,
}

pub fn require_scope(
    headers: &HeaderMap,
    valid_tokens: &[String],
    required_scope: &str,
    require_auth: bool,
) -> Result<AuthContext, StatusCode> {
    if !require_auth {
        return Ok(AuthContext {
            token: "dev".to_string(),
            scopes: vec![required_scope.to_string()],
        });
    }

    let header = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_string();

    let matched = valid_tokens.iter().any(|candidate| candidate == &token);
    if !matched {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(AuthContext {
        token,
        scopes: vec![required_scope.to_string()],
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode};

    use super::require_scope;

    #[test]
    fn rejects_missing_bearer_token_when_auth_required() {
        let headers = HeaderMap::new();
        let result = require_scope(&headers, &["secret".to_string()], "run:read", true);

        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn accepts_matching_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));

        let result = require_scope(&headers, &["secret".to_string()], "run:read", true).unwrap();

        assert_eq!(result.token, "secret");
        assert_eq!(result.scopes, vec!["run:read"]);
    }
}
