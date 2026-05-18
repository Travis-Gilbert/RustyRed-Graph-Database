use axum::http::{HeaderMap, StatusCode};

const ALL_SCOPES: [&str; 15] = [
    "run:write",
    "run:read",
    "context:write",
    "context:read",
    "graph:read",
    "graph:write",
    "admin:read",
    "rustyred:graph:read",
    "rustyred:graph:query",
    "rustyred:graph:context",
    "rustyred:graph:write:propose",
    "rustyred:graph:write:apply",
    "rustyred:graph:index:read",
    "rustyred:graph:admin:verify",
    "rustyred:events:read",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiToken {
    pub token: String,
    pub scopes: Vec<String>,
}

impl ApiToken {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let (token, scopes) = trimmed
            .split_once('=')
            .or_else(|| trimmed.split_once(':'))
            .unwrap_or((trimmed, "*"));
        let scopes = scopes
            .split(['|', ' ', '+'])
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();

        Some(Self {
            token: token.trim().to_string(),
            scopes: if scopes.is_empty() {
                vec!["*".to_string()]
            } else {
                scopes
            },
        })
    }

    fn allows(&self, required_scope: &str) -> bool {
        self.scopes.iter().any(|scope| {
            scope == "*" || scope == required_scope || scope_alias(scope) == required_scope
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthContext {
    pub token: String,
    pub scopes: Vec<String>,
}

pub fn require_scope(
    headers: &HeaderMap,
    valid_tokens: &[ApiToken],
    required_scope: &str,
    require_auth: bool,
) -> Result<AuthContext, StatusCode> {
    if !require_auth {
        return Ok(AuthContext {
            token: "dev".to_string(),
            scopes: ALL_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
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

    let matched = valid_tokens
        .iter()
        .find(|candidate| candidate.token == token)
        .ok_or(StatusCode::FORBIDDEN)?;
    if !matched.allows(required_scope) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(AuthContext {
        token,
        scopes: matched.scopes.clone(),
    })
}

fn scope_alias(scope: &str) -> &str {
    match scope {
        "rustyred:graph:read" | "rustyred:graph:query" | "rustyred:graph:index:read" => "graph:read",
        "rustyred:graph:write:propose" | "rustyred:graph:write:apply" => "graph:write",
        "rustyred:graph:context" => "context:read",
        "rustyred:graph:admin:verify" => "admin:read",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode};

    use super::{require_scope, ApiToken};

    #[test]
    fn rejects_missing_bearer_token_when_auth_required() {
        let headers = HeaderMap::new();
        let tokens = vec![ApiToken {
            token: "secret".to_string(),
            scopes: vec!["run:read".to_string()],
        }];
        let result = require_scope(&headers, &tokens, "run:read", true);

        assert_eq!(result.unwrap_err(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn accepts_matching_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        let tokens = vec![ApiToken {
            token: "secret".to_string(),
            scopes: vec!["run:read".to_string()],
        }];

        let result = require_scope(&headers, &tokens, "run:read", true).unwrap();

        assert_eq!(result.token, "secret");
        assert_eq!(result.scopes, vec!["run:read"]);
    }

    #[test]
    fn rejects_token_without_required_scope() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        let tokens = vec![ApiToken {
            token: "secret".to_string(),
            scopes: vec!["run:read".to_string()],
        }];

        let result = require_scope(&headers, &tokens, "run:write", true);

        assert_eq!(result.unwrap_err(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn parses_scoped_token_from_env_value() {
        let token = ApiToken::parse("secret=run:read|graph:read").unwrap();

        assert_eq!(token.token, "secret");
        assert!(token.allows("run:read"));
        assert!(token.allows("graph:read"));
        assert!(!token.allows("admin:read"));
    }

    #[test]
    fn accepts_rustyred_scope_aliases_for_mcp_tokens() {
        let token = ApiToken::parse("secret=rustyred:graph:read|rustyred:graph:admin:verify").unwrap();

        assert!(token.allows("graph:read"));
        assert!(token.allows("admin:read"));
        assert!(!token.allows("graph:write"));
    }
}
