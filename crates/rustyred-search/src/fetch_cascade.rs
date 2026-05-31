use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, ETAG, LAST_MODIFIED};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{RustyWebError, RustyWebResult};

pub const PROMOTION_BODY_THRESHOLD: usize = 512;
pub const CLOUDFLARE_INTERSTITIAL_THRESHOLD: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum FetchTier {
    Http2 = 1,
    Impersonate = 2,
    Rendered = 3,
}

impl FetchTier {
    fn max_supported() -> Self {
        FetchTier::Http2
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FetchTierResult {
    pub tier_used: FetchTier,
    pub html_bytes: Vec<u8>,
    pub http_status: u16,
    pub content_type: String,
    pub etag: String,
    pub last_modified: String,
    pub final_url: String,
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchCascadeOptions {
    pub user_agent: String,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DomainTierState {
    tiers: Arc<Mutex<HashMap<String, FetchTier>>>,
}

impl DomainTierState {
    pub fn tier_for(&self, domain: &str) -> FetchTier {
        self.tiers
            .lock()
            .ok()
            .and_then(|tiers| tiers.get(domain).copied())
            .unwrap_or(FetchTier::Http2)
    }

    pub fn promote(&self, domain: &str, tier: FetchTier) {
        if let Ok(mut tiers) = self.tiers.lock() {
            let current = tiers.entry(domain.to_string()).or_insert(FetchTier::Http2);
            if tier > *current {
                *current = tier;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct FetchCascade {
    client: reqwest::Client,
    state: DomainTierState,
}

impl FetchCascade {
    pub fn new(options: FetchCascadeOptions) -> RustyWebResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(options.user_agent)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(options.timeout_seconds))
            .build()
            .map_err(|err| RustyWebError::Fetch {
                url: "<client>".to_string(),
                reason: err.to_string(),
            })?;
        Ok(Self {
            client,
            state: DomainTierState::default(),
        })
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub async fn fetch_with_promotion(
        &self,
        canonical_url: &str,
        max_bytes: usize,
    ) -> RustyWebResult<FetchTierResult> {
        let domain = domain_for_url(canonical_url)?;
        let desired_tier = self.state.tier_for(&domain);
        let supported_tier = desired_tier.min(FetchTier::max_supported());
        let result = self
            .fetch_http2(canonical_url, max_bytes, supported_tier)
            .await?;

        if should_promote(&result) {
            self.state.promote(&domain, FetchTier::Impersonate);
        }

        Ok(result)
    }

    async fn fetch_http2(
        &self,
        canonical_url: &str,
        max_bytes: usize,
        tier_used: FetchTier,
    ) -> RustyWebResult<FetchTierResult> {
        let response =
            self.client
                .get(canonical_url)
                .send()
                .await
                .map_err(|err| RustyWebError::Fetch {
                    url: canonical_url.to_string(),
                    reason: err.to_string(),
                })?;
        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let headers = response.headers().clone();
        let content_type = header_string(&headers, CONTENT_TYPE.as_str())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let etag = header_string(&headers, ETAG.as_str()).unwrap_or_default();
        let last_modified = header_string(&headers, LAST_MODIFIED.as_str()).unwrap_or_default();
        let html_bytes = read_limited_body(response, canonical_url, max_bytes).await?;

        Ok(FetchTierResult {
            tier_used,
            html_bytes,
            http_status: status,
            content_type,
            etag,
            last_modified,
            final_url,
            error: String::new(),
        })
    }
}

pub fn should_promote(result: &FetchTierResult) -> bool {
    match result.http_status {
        0 => false,
        403 | 429 | 503 => true,
        status if (200..300).contains(&status) => {
            let size = result.html_bytes.len();
            size < PROMOTION_BODY_THRESHOLD
                || (size < CLOUDFLARE_INTERSTITIAL_THRESHOLD
                    && (contains_bytes(&result.html_bytes, b"Just a moment")
                        || contains_bytes(&result.html_bytes, b"cf-browser-verification")))
        }
        _ => false,
    }
}

async fn read_limited_body(
    mut response: reqwest::Response,
    url: &str,
    limit: usize,
) -> RustyWebResult<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| RustyWebError::Fetch {
        url: url.to_string(),
        reason: err.to_string(),
    })? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(RustyWebError::BodyLimitExceeded {
                url: url.to_string(),
                limit,
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn header_string(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn domain_for_url(raw: &str) -> RustyWebResult<String> {
    let url = Url::parse(raw).map_err(|err| RustyWebError::InvalidUrl {
        url: raw.to_string(),
        reason: err.to_string(),
    })?;
    url.host_str()
        .map(|host| host.to_ascii_lowercase())
        .ok_or_else(|| RustyWebError::InvalidUrl {
            url: raw.to_string(),
            reason: "missing host".to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(status: u16, body: &[u8]) -> FetchTierResult {
        FetchTierResult {
            tier_used: FetchTier::Http2,
            html_bytes: body.to_vec(),
            http_status: status,
            content_type: "text/html".to_string(),
            etag: String::new(),
            last_modified: String::new(),
            final_url: "https://example.com/".to_string(),
            error: String::new(),
        }
    }

    #[test]
    fn promotion_triggers_match_the_theseus_fetch_contract() {
        assert!(should_promote(&result(403, b"forbidden")));
        assert!(should_promote(&result(429, b"slow down")));
        assert!(should_promote(&result(503, b"unavailable")));
        assert!(should_promote(&result(200, b"tiny")));
        assert!(should_promote(&result(200, b"Just a moment")));
        assert!(should_promote(&result(200, b"cf-browser-verification")));
    }

    #[test]
    fn promotion_does_not_fire_for_transport_failure_or_healthy_body() {
        let healthy = vec![b'x'; PROMOTION_BODY_THRESHOLD + 16];
        assert!(!should_promote(&result(0, b"")));
        assert!(!should_promote(&result(200, &healthy)));
        assert!(!should_promote(&result(404, b"not found")));
    }

    #[test]
    fn domain_tier_state_only_promotes() {
        let state = DomainTierState::default();
        assert_eq!(state.tier_for("example.com"), FetchTier::Http2);
        state.promote("example.com", FetchTier::Impersonate);
        state.promote("example.com", FetchTier::Http2);
        assert_eq!(state.tier_for("example.com"), FetchTier::Impersonate);
    }
}
