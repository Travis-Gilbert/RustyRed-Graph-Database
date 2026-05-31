use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use texting_robots::Robot;
use url::Url;

use crate::{RustyWebError, RustyWebResult};

const DEFAULT_ROBOTS_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RobotsPolicyState {
    Parsed,
    MissingAllowAll,
    UnavailableAllowAll,
    MalformedAllowAll,
    ForbiddenDisallowAll,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RobotsDecision {
    pub allowed: bool,
    pub state: RobotsPolicyState,
    pub crawl_delay_seconds: Option<f32>,
    pub sitemaps: Vec<String>,
    pub reason: String,
}

#[derive(Clone)]
enum CachedRobotsPolicy {
    AllowAll {
        state: RobotsPolicyState,
        reason: String,
    },
    DisallowAll {
        state: RobotsPolicyState,
        reason: String,
    },
    Parsed {
        robot: Arc<Robot>,
    },
}

#[derive(Clone)]
struct RobotsCacheEntry {
    expires_at: Instant,
    policy: CachedRobotsPolicy,
}

#[derive(Clone)]
pub struct RobotsCache {
    ttl: Duration,
    entries: Arc<Mutex<HashMap<String, RobotsCacheEntry>>>,
}

impl Default for RobotsCache {
    fn default() -> Self {
        Self::new(DEFAULT_ROBOTS_TTL)
    }
}

impl RobotsCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn check(
        &self,
        client: &reqwest::Client,
        url: &str,
        user_agent: &str,
    ) -> RustyWebResult<RobotsDecision> {
        let key = robots_cache_key(url)?;
        if let Some(policy) = self.cached_policy(&key) {
            return Ok(policy.decision_for(url));
        }

        let policy = fetch_policy(client, url, user_agent).await;
        self.store_policy(key, policy.clone());
        Ok(policy.decision_for(url))
    }

    fn cached_policy(&self, key: &str) -> Option<CachedRobotsPolicy> {
        let now = Instant::now();
        let mut entries = self.entries.lock().ok()?;
        match entries.get(key) {
            Some(entry) if entry.expires_at > now => Some(entry.policy.clone()),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    fn store_policy(&self, key: String, policy: CachedRobotsPolicy) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                key,
                RobotsCacheEntry {
                    expires_at: Instant::now() + self.ttl,
                    policy,
                },
            );
        }
    }
}

impl CachedRobotsPolicy {
    fn decision_for(&self, url: &str) -> RobotsDecision {
        match self {
            Self::AllowAll { state, reason } => RobotsDecision {
                allowed: true,
                state: *state,
                crawl_delay_seconds: None,
                sitemaps: Vec::new(),
                reason: reason.clone(),
            },
            Self::DisallowAll { state, reason } => RobotsDecision {
                allowed: false,
                state: *state,
                crawl_delay_seconds: None,
                sitemaps: Vec::new(),
                reason: reason.clone(),
            },
            Self::Parsed { robot } => RobotsDecision {
                allowed: robot.allowed(url),
                state: RobotsPolicyState::Parsed,
                crawl_delay_seconds: robot.delay,
                sitemaps: robot.sitemaps.clone(),
                reason: "robots.txt parsed".to_string(),
            },
        }
    }
}

pub fn global_robots_cache() -> &'static RobotsCache {
    static CACHE: OnceLock<RobotsCache> = OnceLock::new();
    CACHE.get_or_init(RobotsCache::default)
}

pub fn crawl_delay_duration(decision: &RobotsDecision) -> Option<Duration> {
    decision
        .crawl_delay_seconds
        .filter(|delay| delay.is_finite() && *delay > 0.0)
        .map(Duration::from_secs_f32)
}

async fn fetch_policy(client: &reqwest::Client, url: &str, user_agent: &str) -> CachedRobotsPolicy {
    let robots_url = match texting_robots::get_robots_url(url) {
        Ok(url) => url,
        Err(error) => {
            return CachedRobotsPolicy::AllowAll {
                state: RobotsPolicyState::UnavailableAllowAll,
                reason: format!("robots url unavailable: {error}"),
            };
        }
    };

    let response = match client.get(&robots_url).send().await {
        Ok(response) => response,
        Err(error) => {
            return CachedRobotsPolicy::AllowAll {
                state: RobotsPolicyState::UnavailableAllowAll,
                reason: format!("robots fetch failed: {error}"),
            };
        }
    };
    let status = response.status().as_u16();
    if matches!(status, 401 | 403) {
        return CachedRobotsPolicy::DisallowAll {
            state: RobotsPolicyState::ForbiddenDisallowAll,
            reason: format!("robots.txt status {status} disallows crawl"),
        };
    }
    if status == 404 {
        return CachedRobotsPolicy::AllowAll {
            state: RobotsPolicyState::MissingAllowAll,
            reason: "robots.txt missing".to_string(),
        };
    }
    if !response.status().is_success() {
        return CachedRobotsPolicy::AllowAll {
            state: RobotsPolicyState::UnavailableAllowAll,
            reason: format!("robots.txt status {status} treated as unavailable"),
        };
    }

    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            return CachedRobotsPolicy::AllowAll {
                state: RobotsPolicyState::UnavailableAllowAll,
                reason: format!("robots body failed: {error}"),
            };
        }
    };
    match Robot::new(user_agent, &body) {
        Ok(robot) => CachedRobotsPolicy::Parsed {
            robot: Arc::new(robot),
        },
        Err(error) => CachedRobotsPolicy::AllowAll {
            state: RobotsPolicyState::MalformedAllowAll,
            reason: format!("robots malformed: {error}"),
        },
    }
}

fn robots_cache_key(raw: &str) -> RustyWebResult<String> {
    let url = Url::parse(raw).map_err(|err| RustyWebError::InvalidUrl {
        url: raw.to_string(),
        reason: err.to_string(),
    })?;
    let host = url
        .host_str()
        .ok_or_else(|| RustyWebError::InvalidUrl {
            url: raw.to_string(),
            reason: "missing host".to_string(),
        })?
        .to_ascii_lowercase();
    let port = url.port_or_known_default().unwrap_or(0);
    Ok(format!("{}://{}:{}", url.scheme(), host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsed_policy_allows_and_disallows_by_path() {
        let robot = Robot::new("RustyWeb", b"User-agent: *\nDisallow: /private\nAllow: /\n")
            .expect("valid robots");
        let policy = CachedRobotsPolicy::Parsed {
            robot: Arc::new(robot),
        };
        assert!(!policy.decision_for("https://example.com/private").allowed);
        assert!(policy.decision_for("https://example.com/public").allowed);
    }

    #[test]
    fn crawl_delay_maps_to_duration() {
        let decision = RobotsDecision {
            allowed: true,
            state: RobotsPolicyState::Parsed,
            crawl_delay_seconds: Some(0.25),
            sitemaps: Vec::new(),
            reason: String::new(),
        };
        assert_eq!(
            crawl_delay_duration(&decision),
            Some(Duration::from_millis(250))
        );
    }

    #[test]
    fn cache_key_separates_scheme_host_and_port() {
        assert_eq!(
            robots_cache_key("https://Example.com/path").unwrap(),
            "https://example.com:443"
        );
        assert_eq!(
            robots_cache_key("http://example.com:8080/path").unwrap(),
            "http://example.com:8080"
        );
    }
}
