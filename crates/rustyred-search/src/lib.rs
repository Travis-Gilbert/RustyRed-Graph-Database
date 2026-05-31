use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::net::IpAddr;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lol_html::{element, HtmlRewriter, Settings};
use rustyred_core::{
    EdgeRecord, GraphMutation, GraphMutationBatch, GraphStore, GraphStoreError, GraphStoreResult,
    GraphWriteResult, NodeRecord, Provenance,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use url::Url;

pub const LABEL_CRAWL_RUN: &str = "CrawlRun";
pub const LABEL_FETCH_ATTEMPT: &str = "FetchAttempt";
pub const LABEL_DOMAIN: &str = "Domain";
pub const LABEL_PAGE: &str = "Page";
pub const LABEL_CONTENT_SNAPSHOT: &str = "ContentSnapshot";
pub const LABEL_ROBOTS_POLICY: &str = "RobotsPolicy";
pub const LABEL_CRAWL_RECEIPT: &str = "CrawlReceipt";
pub const LABEL_DISCOVERY_SEED: &str = "DiscoverySeed";
pub const LABEL_WEB_COMMONS_PEER: &str = "WebCommonsPeer";
pub const LABEL_WEB_COMMONS_ATTESTATION: &str = "WebCommonsAttestation";

pub const EDGE_FETCHED: &str = "FETCHED";
pub const EDGE_RESULTED_IN: &str = "RESULTED_IN";
pub const EDGE_HAS_SNAPSHOT: &str = "HAS_SNAPSHOT";
pub const EDGE_LINKS_TO: &str = "LINKS_TO";
pub const EDGE_ON_DOMAIN: &str = "ON_DOMAIN";
pub const EDGE_ROBOTS_APPLIED: &str = "ROBOTS_APPLIED";
pub const EDGE_CANONICAL_OF: &str = "CANONICAL_OF";
pub const EDGE_EMITTED_RECEIPT: &str = "EMITTED_RECEIPT";
pub const EDGE_SEEDED: &str = "SEEDED";
pub const EDGE_SUBMITTED_BY: &str = "SUBMITTED_BY";
pub const EDGE_ATTESTS_PAGE: &str = "ATTESTS_PAGE";

pub const TTL_PROPERTY: &str = "_ttl_expires_at_ms";
pub const WEB_COMMONS_PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_WEB_COMMONS_SNAPSHOT_TEXT_BYTES: usize = 4096;

pub type FetchedPage = FixturePage;

pub mod fetch_cascade;
pub use fetch_cascade::{
    should_promote, FetchCascade, FetchCascadeOptions, FetchTier, FetchTierResult,
};

pub mod robots;
pub use robots::{
    crawl_delay_duration, global_robots_cache, RobotsCache, RobotsDecision, RobotsPolicyState,
};

pub mod source_class;
pub use source_class::{
    classify_url, profile_for, profile_for_url, CitationStrategy, ExtractionProfile, SourceClass,
};

pub mod trigger_gate;
pub use trigger_gate::{evaluate_trigger_gate, CrawlDial, TriggerGateConfig, TriggerGateDecision};

// Substrate-native local graph search (the READ seam). See search.rs.
pub mod search;
pub use search::{search_substrate, SearchHit, SearchLink, SearchOptions, SubstrateSearch};

// The browser's SERP: render a search as a node-and-edge graph page. See serp.rs.
pub mod serp;
pub use serp::{render_search_page, render_serp_html, serp_payload_json};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrawlConfig {
    pub run_id: String,
    pub namespace: String,
    pub user_agent: String,
}

impl Default for CrawlConfig {
    fn default() -> Self {
        Self {
            run_id: "fixture-run".to_string(),
            namespace: "link".to_string(),
            user_agent: "RustyWeb/0.1 fixture".to_string(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FixturePage {
    pub url: String,
    pub status: u16,
    pub body: String,
    pub content_type: String,
    pub fetched_at: String,
}

impl FixturePage {
    pub fn html(url: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            status: 200,
            body: body.into(),
            content_type: "text/html; charset=utf-8".to_string(),
            fetched_at: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrawlCounters {
    pub fetched_pages: usize,
    pub discovered_pages: usize,
    pub domains: usize,
    pub snapshots: usize,
    pub links: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrawlBudget {
    pub max_pages: usize,
    pub max_seconds: u64,
    pub max_depth: usize,
    pub max_bytes: usize,
}

impl Default for CrawlBudget {
    fn default() -> Self {
        Self {
            max_pages: 25,
            max_seconds: 30,
            max_depth: 2,
            max_bytes: 5 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrawlScope {
    pub namespace: String,
    pub follow_offsite: bool,
    pub ttl_expires_at_ms: Option<i64>,
    pub source_graph: String,
    pub source_license: String,
    pub federable: bool,
    pub actor_id: String,
}

impl Default for CrawlScope {
    fn default() -> Self {
        Self {
            namespace: "link".to_string(),
            follow_offsite: true,
            ttl_expires_at_ms: None,
            source_graph: "rustyweb_crawler".to_string(),
            source_license: "unknown".to_string(),
            federable: false,
            actor_id: String::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrawlRequest {
    pub run_id: String,
    pub seeds: Vec<String>,
    pub budget: CrawlBudget,
    pub scope: CrawlScope,
}

impl CrawlRequest {
    pub fn new(run_id: impl Into<String>, seeds: Vec<String>) -> Self {
        Self {
            run_id: run_id.into(),
            seeds,
            budget: CrawlBudget::default(),
            scope: CrawlScope::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UrlGuardPolicy {
    pub allow_loopback: bool,
    pub allow_private_networks: bool,
    pub block_metadata_services: bool,
}

impl Default for UrlGuardPolicy {
    fn default() -> Self {
        Self {
            allow_loopback: false,
            allow_private_networks: false,
            block_metadata_services: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LiveFetchOptions {
    pub user_agent: String,
    pub timeout_seconds: u64,
    pub guard_policy: UrlGuardPolicy,
    pub respect_robots: bool,
}

impl Default for LiveFetchOptions {
    fn default() -> Self {
        Self {
            user_agent: "RustyWeb/0.2 live".to_string(),
            timeout_seconds: 10,
            guard_policy: UrlGuardPolicy::default(),
            respect_robots: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrawlReceipt {
    pub receipt_id: String,
    pub run_id: String,
    pub namespace: String,
    pub status: String,
    pub seed_count: usize,
    pub counters: CrawlCounters,
    pub graph_delta_hash: String,
    pub budget: CrawlBudget,
    pub scope: CrawlScope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrawlRunOutput {
    pub graph: CrawlGraph,
    pub receipt: CrawlReceipt,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WebCommonsFragmentOptions {
    pub include_provenance: bool,
    pub snapshot_text_bytes: usize,
}

impl Default for WebCommonsFragmentOptions {
    fn default() -> Self {
        Self {
            include_provenance: false,
            snapshot_text_bytes: DEFAULT_WEB_COMMONS_SNAPSHOT_TEXT_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WebCommonsFragment {
    pub protocol_version: u32,
    pub peer_id: String,
    pub graph_delta_hash: String,
    pub pages: Vec<PageRecord>,
    pub snapshots: Vec<SnapshotRecord>,
    pub domains: Vec<DomainRecord>,
    pub edges: Vec<LinkEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<CrawlProvenance>,
    #[serde(default)]
    pub source_licenses: Vec<SourceLicense>,
    #[serde(default)]
    pub signature: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WebCommonsUnsignedFragment {
    pub protocol_version: u32,
    pub peer_id: String,
    pub graph_delta_hash: String,
    pub pages: Vec<PageRecord>,
    pub snapshots: Vec<SnapshotRecord>,
    pub domains: Vec<DomainRecord>,
    pub edges: Vec<LinkEdge>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<CrawlProvenance>,
    #[serde(default)]
    pub source_licenses: Vec<SourceLicense>,
}

impl WebCommonsFragment {
    pub fn unsigned_payload(&self) -> WebCommonsUnsignedFragment {
        WebCommonsUnsignedFragment {
            protocol_version: self.protocol_version,
            peer_id: self.peer_id.clone(),
            graph_delta_hash: self.graph_delta_hash.clone(),
            pages: self.pages.clone(),
            snapshots: self.snapshots.clone(),
            domains: self.domains.clone(),
            edges: self.edges.clone(),
            provenance: self.provenance.clone(),
            source_licenses: self.source_licenses.clone(),
        }
    }

    pub fn signing_bytes(&self) -> RustyWebResult<Vec<u8>> {
        serde_json::to_vec(&self.unsigned_payload()).map_err(|err| RustyWebError::InvalidFragment {
            reason: format!("failed to canonicalize Web Commons fragment: {err}"),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageRecord {
    pub id: String,
    pub url: String,
    pub domain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
    pub source_class: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotRecord {
    pub id: String,
    pub page_id: String,
    pub content_hash: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DomainRecord {
    pub domain: String,
    pub page_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinkEdge {
    pub from_page_id: String,
    pub to_page_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CrawlProvenance {
    pub run_id: String,
    pub seeds: Vec<String>,
    pub budget: CrawlBudget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceLicense {
    pub domain: String,
    pub source_graph: String,
    pub source_license: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WebCommonsPageDisposition {
    pub page_id: String,
    pub url: String,
    pub disposition: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WebCommonsReceipt {
    pub accepted: bool,
    pub peer_id: String,
    pub graph_delta_hash: String,
    pub accepted_pages: usize,
    pub dropped_pages: usize,
    pub dispositions: Vec<WebCommonsPageDisposition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WebCommonsIngestPlan {
    pub batch: GraphMutationBatch,
    pub receipt: WebCommonsReceipt,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrawlGraph {
    pub run_id: String,
    pub namespace: String,
    pub counters: CrawlCounters,
    pub batch: GraphMutationBatch,
}

impl CrawlGraph {
    pub fn apply_to_store(
        &self,
        store: &mut impl GraphStore,
    ) -> GraphStoreResult<Vec<GraphWriteResult>> {
        apply_batch_to_store(store, &self.batch)
    }

    pub fn nodes(&self) -> Vec<NodeRecord> {
        self.batch
            .mutations
            .iter()
            .filter_map(|mutation| match mutation {
                GraphMutation::NodeUpsert(node) => Some(node.clone()),
                GraphMutation::EdgeUpsert(_) => None,
            })
            .collect()
    }

    pub fn edges(&self) -> Vec<EdgeRecord> {
        self.batch
            .mutations
            .iter()
            .filter_map(|mutation| match mutation {
                GraphMutation::NodeUpsert(_) => None,
                GraphMutation::EdgeUpsert(edge) => Some(edge.clone()),
            })
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RustyWebError {
    InvalidUrl { url: String, reason: String },
    HtmlParse { reason: String },
    Fetch { url: String, reason: String },
    BodyLimitExceeded { url: String, limit: usize },
    EmptySeeds,
    InvalidBudget { reason: String },
    BlockedUrl { url: String, reason: String },
    InvalidFragment { reason: String },
}

impl fmt::Display for RustyWebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl { url, reason } => {
                write!(f, "invalid URL {url:?}: {reason}")
            }
            Self::HtmlParse { reason } => write!(f, "HTML parse failed: {reason}"),
            Self::Fetch { url, reason } => write!(f, "fetch failed for {url:?}: {reason}"),
            Self::BodyLimitExceeded { url, limit } => {
                write!(f, "fetch body for {url:?} exceeded {limit} bytes")
            }
            Self::EmptySeeds => write!(f, "crawl request requires at least one seed URL"),
            Self::InvalidBudget { reason } => write!(f, "invalid crawl budget: {reason}"),
            Self::BlockedUrl { url, reason } => write!(f, "blocked URL {url:?}: {reason}"),
            Self::InvalidFragment { reason } => write!(f, "invalid Web Commons fragment: {reason}"),
        }
    }
}

impl std::error::Error for RustyWebError {}

pub type RustyWebResult<T> = Result<T, RustyWebError>;

pub fn build_fixture_crawl_graph(
    config: CrawlConfig,
    pages: &[FixturePage],
) -> RustyWebResult<CrawlGraph> {
    let run_node_id = crawl_run_id(&config.run_id);
    let mut nodes: BTreeMap<String, NodeRecord> = BTreeMap::new();
    let mut edges: BTreeMap<String, EdgeRecord> = BTreeMap::new();
    let mut fetched_page_ids = BTreeSet::new();
    let mut snapshot_ids = BTreeSet::new();
    let mut link_count = 0usize;

    insert_node(
        &mut nodes,
        NodeRecord::new(
            run_node_id.clone(),
            [LABEL_CRAWL_RUN],
            json!({
                "run_id": config.run_id,
                "namespace": config.namespace,
                "user_agent": config.user_agent,
                "engine": "rustyweb_fixture_v0"
            }),
        ),
    );

    for (index, fixture) in pages.iter().enumerate() {
        let canonical_url = canonicalize_url(&fixture.url)?;
        let page_node_id = page_id(&canonical_url);
        let domain = domain_for_url(&canonical_url)?;
        let domain_node_id = domain_id(&domain);
        let robots_id = robots_policy_id(&domain);
        let attempt_id = fetch_attempt_id(&config.run_id, index, &fixture.url);
        let content_hash = blake3_hash(fixture.body.as_bytes());
        let snapshot_id = content_snapshot_id(&content_hash);
        let fetched_at = if fixture.fetched_at.is_empty() {
            Value::Null
        } else {
            Value::String(fixture.fetched_at.clone())
        };

        fetched_page_ids.insert(page_node_id.clone());
        snapshot_ids.insert(snapshot_id.clone());

        insert_domain_node(&mut nodes, &domain, &domain_node_id, &config.namespace);
        insert_robots_node(&mut nodes, &domain, &robots_id, &config.namespace);
        insert_page_node(
            &mut nodes,
            &canonical_url,
            &page_node_id,
            &domain,
            &config.namespace,
            "fetched",
        );
        insert_node(
            &mut nodes,
            NodeRecord::new(
                attempt_id.clone(),
                [LABEL_FETCH_ATTEMPT],
                json!({
                    "url": fixture.url,
                    "canonical_url": canonical_url,
                    "status": fixture.status,
                    "content_type": fixture.content_type,
                    "body_bytes": fixture.body.len(),
                    "success": (200..400).contains(&fixture.status),
                    "fetched_at": fetched_at,
                    "namespace": config.namespace,
                }),
            ),
        );
        insert_node(
            &mut nodes,
            NodeRecord::new(
                snapshot_id.clone(),
                [LABEL_CONTENT_SNAPSHOT],
                json!({
                    "content_hash": content_hash,
                    "hash_algorithm": "blake3",
                    "byte_len": fixture.body.len(),
                    "text": html_to_text(&fixture.body),
                    "namespace": config.namespace,
                }),
            ),
        );

        insert_edge(
            &mut edges,
            &run_node_id,
            EDGE_FETCHED,
            &attempt_id,
            json!({}),
        );
        insert_edge(
            &mut edges,
            &attempt_id,
            EDGE_RESULTED_IN,
            &page_node_id,
            json!({"status": fixture.status}),
        );
        insert_edge(
            &mut edges,
            &page_node_id,
            EDGE_HAS_SNAPSHOT,
            &snapshot_id,
            json!({}),
        );
        insert_edge(
            &mut edges,
            &page_node_id,
            EDGE_ON_DOMAIN,
            &domain_node_id,
            json!({}),
        );
        insert_edge(
            &mut edges,
            &attempt_id,
            EDGE_ROBOTS_APPLIED,
            &robots_id,
            json!({"policy": "fixture_assumed_allowed"}),
        );

        if fixture.url != canonical_url {
            let alias_id = page_alias_id(&fixture.url);
            insert_node(
                &mut nodes,
                NodeRecord::new(
                    alias_id.clone(),
                    [LABEL_PAGE],
                    json!({
                        "url": fixture.url,
                        "canonical_url": canonical_url,
                        "page_state": "alias",
                        "namespace": config.namespace,
                    }),
                ),
            );
            insert_edge(
                &mut edges,
                &alias_id,
                EDGE_CANONICAL_OF,
                &page_node_id,
                json!({}),
            );
        }

        for target_url in extract_links_for_url(&canonical_url, &fixture.body)? {
            let target_domain = domain_for_url(&target_url)?;
            let target_domain_node_id = domain_id(&target_domain);
            let target_page_node_id = page_id(&target_url);
            insert_domain_node(
                &mut nodes,
                &target_domain,
                &target_domain_node_id,
                &config.namespace,
            );
            insert_robots_node(
                &mut nodes,
                &target_domain,
                &robots_policy_id(&target_domain),
                &config.namespace,
            );
            insert_page_node(
                &mut nodes,
                &target_url,
                &target_page_node_id,
                &target_domain,
                &config.namespace,
                "discovered",
            );
            insert_edge(
                &mut edges,
                &target_page_node_id,
                EDGE_ON_DOMAIN,
                &target_domain_node_id,
                json!({}),
            );
            if insert_edge(
                &mut edges,
                &page_node_id,
                EDGE_LINKS_TO,
                &target_page_node_id,
                json!({"source": "html_a_href"}),
            ) {
                link_count += 1;
            }
        }
    }

    let mut mutations = Vec::with_capacity(nodes.len() + edges.len());
    mutations.extend(nodes.values().cloned().map(GraphMutation::NodeUpsert));
    mutations.extend(edges.values().cloned().map(GraphMutation::EdgeUpsert));

    let discovered_pages = nodes
        .values()
        .filter(|node| node.labels.iter().any(|label| label == LABEL_PAGE))
        .count();
    let domains = nodes
        .values()
        .filter(|node| node.labels.iter().any(|label| label == LABEL_DOMAIN))
        .count();

    Ok(CrawlGraph {
        run_id: config.run_id,
        namespace: config.namespace,
        counters: CrawlCounters {
            fetched_pages: fetched_page_ids.len(),
            discovered_pages,
            domains,
            snapshots: snapshot_ids.len(),
            links: link_count,
        },
        batch: GraphMutationBatch { mutations },
    })
}

pub fn build_v2_fixture_crawl(
    request: CrawlRequest,
    pages: &[FetchedPage],
) -> RustyWebResult<CrawlRunOutput> {
    build_v2_fixture_crawl_with_policy(request, pages, &UrlGuardPolicy::default())
}

pub fn build_v2_fixture_crawl_with_policy(
    request: CrawlRequest,
    pages: &[FetchedPage],
    guard_policy: &UrlGuardPolicy,
) -> RustyWebResult<CrawlRunOutput> {
    let guarded_seeds = validate_crawl_request(&request, guard_policy)?;
    let (selected_pages, budget_limited) = select_budgeted_pages(&request.budget, pages);
    let status = if budget_limited {
        "budget_limited"
    } else {
        "completed"
    }
    .to_string();
    let mut graph = build_fixture_crawl_graph(
        CrawlConfig {
            run_id: request.run_id.clone(),
            namespace: request.scope.namespace.clone(),
            user_agent: "RustyWeb/0.2 fixture".to_string(),
        },
        &selected_pages,
    )?;

    add_seed_nodes(&mut graph, &request, &guarded_seeds);
    annotate_graph_for_scope(&mut graph, &request.scope);
    let graph_delta_hash = graph_delta_hash(&graph.batch);
    let receipt_id = crawl_receipt_id(&request.run_id, &graph_delta_hash);
    let receipt = CrawlReceipt {
        receipt_id,
        run_id: request.run_id.clone(),
        namespace: request.scope.namespace.clone(),
        status,
        seed_count: guarded_seeds.len(),
        counters: graph.counters.clone(),
        graph_delta_hash,
        budget: request.budget.clone(),
        scope: request.scope.clone(),
    };
    add_receipt_node(&mut graph, &receipt);

    Ok(CrawlRunOutput { graph, receipt })
}

pub async fn run_live_crawl(request: CrawlRequest) -> RustyWebResult<CrawlRunOutput> {
    run_live_crawl_with_options(request, &LiveFetchOptions::default()).await
}

pub async fn run_live_crawl_with_options(
    request: CrawlRequest,
    options: &LiveFetchOptions,
) -> RustyWebResult<CrawlRunOutput> {
    let pages = fetch_seed_pages(&request, options).await?;
    build_v2_fixture_crawl_with_policy(request, &pages, &options.guard_policy)
}

pub fn build_web_commons_fragment(
    output: &CrawlRunOutput,
    request: &CrawlRequest,
    peer_id: impl Into<String>,
    options: &WebCommonsFragmentOptions,
) -> RustyWebResult<WebCommonsFragment> {
    let peer_id = peer_id.into();
    if peer_id.trim().is_empty() {
        return Err(RustyWebError::InvalidFragment {
            reason: "peer_id is required".to_string(),
        });
    }

    let nodes = output.graph.nodes();
    let edges = output.graph.edges();
    let node_by_id = nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<BTreeMap<_, _>>();
    let mut attempt_by_page: BTreeMap<String, &NodeRecord> = BTreeMap::new();
    let mut snapshot_page: BTreeMap<String, String> = BTreeMap::new();

    for edge in &edges {
        if edge.edge_type == EDGE_RESULTED_IN {
            if let Some(attempt) = node_by_id.get(&edge.from_id) {
                attempt_by_page.insert(edge.to_id.clone(), *attempt);
            }
        } else if edge.edge_type == EDGE_HAS_SNAPSHOT {
            snapshot_page.insert(edge.to_id.clone(), edge.from_id.clone());
        }
    }

    let mut pages = Vec::new();
    let mut page_domains: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut licenses: BTreeMap<String, SourceLicense> = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| node.labels.iter().any(|label| label == LABEL_PAGE))
    {
        let Some(url) = property_string(&node.properties, "url")
            .or_else(|| property_string(&node.properties, "canonical_url"))
        else {
            continue;
        };
        let domain = property_string(&node.properties, "domain")
            .unwrap_or_else(|| domain_for_url(&url).unwrap_or_default());
        let source_class = property_string(&node.properties, "source_class").unwrap_or_else(|| {
            Url::parse(&url)
                .ok()
                .map(|parsed| classify_url(&parsed).as_str().to_string())
                .unwrap_or_else(|| SourceClass::Unknown.as_str().to_string())
        });
        let attempt = attempt_by_page.get(&node.id).copied();
        let status = attempt.and_then(|attempt| property_u16(&attempt.properties, "status"));
        let fetched_at =
            attempt.and_then(|attempt| property_string(&attempt.properties, "fetched_at"));
        pages.push(PageRecord {
            id: node.id.clone(),
            url: url.clone(),
            domain: domain.clone(),
            title: property_string(&node.properties, "title"),
            status,
            fetched_at,
            source_class,
        });
        page_domains
            .entry(domain.clone())
            .or_default()
            .push(node.id.clone());
        licenses.entry(domain).or_insert_with(|| SourceLicense {
            domain: property_string(&node.properties, "domain").unwrap_or_default(),
            source_graph: property_string(&node.properties, "source_graph")
                .unwrap_or_else(|| request.scope.source_graph.clone()),
            source_license: property_string(&node.properties, "source_license")
                .unwrap_or_else(|| request.scope.source_license.clone()),
        });
    }

    let mut snapshots = Vec::new();
    for node in nodes.iter().filter(|node| {
        node.labels
            .iter()
            .any(|label| label == LABEL_CONTENT_SNAPSHOT)
    }) {
        let Some(page_id) = snapshot_page.get(&node.id).cloned() else {
            continue;
        };
        let content_hash = property_string(&node.properties, "content_hash").unwrap_or_else(|| {
            node.id
                .strip_prefix("content_snapshot:")
                .unwrap_or(&node.id)
                .to_string()
        });
        let content_type = attempt_by_page
            .get(&page_id)
            .and_then(|attempt| property_string(&attempt.properties, "content_type"));
        snapshots.push(SnapshotRecord {
            id: node.id.clone(),
            page_id,
            content_hash,
            text: bounded_text(
                property_string(&node.properties, "text").unwrap_or_default(),
                options.snapshot_text_bytes,
            ),
            content_type,
        });
    }

    let mut domains = page_domains
        .into_iter()
        .map(|(domain, mut page_ids)| {
            page_ids.sort();
            page_ids.dedup();
            DomainRecord { domain, page_ids }
        })
        .collect::<Vec<_>>();
    let mut link_edges = edges
        .iter()
        .filter(|edge| edge.edge_type == EDGE_LINKS_TO)
        .map(|edge| LinkEdge {
            from_page_id: edge.from_id.clone(),
            to_page_id: edge.to_id.clone(),
            anchor: property_string(&edge.properties, "anchor"),
        })
        .collect::<Vec<_>>();
    let mut source_licenses = licenses.into_values().collect::<Vec<_>>();

    pages.sort_by(|left, right| left.id.cmp(&right.id));
    snapshots.sort_by(|left, right| left.id.cmp(&right.id));
    domains.sort_by(|left, right| left.domain.cmp(&right.domain));
    link_edges.sort_by(|left, right| {
        left.from_page_id
            .cmp(&right.from_page_id)
            .then_with(|| left.to_page_id.cmp(&right.to_page_id))
    });
    source_licenses.sort_by(|left, right| left.domain.cmp(&right.domain));

    Ok(WebCommonsFragment {
        protocol_version: WEB_COMMONS_PROTOCOL_VERSION,
        peer_id,
        graph_delta_hash: output.receipt.graph_delta_hash.clone(),
        pages,
        snapshots,
        domains,
        edges: link_edges,
        provenance: options.include_provenance.then(|| CrawlProvenance {
            run_id: request.run_id.clone(),
            seeds: request.seeds.clone(),
            budget: request.budget.clone(),
            actor_id: (!request.scope.actor_id.is_empty()).then(|| request.scope.actor_id.clone()),
        }),
        source_licenses,
        signature: String::new(),
    })
}

pub fn build_web_commons_ingest_plan(
    fragment: &WebCommonsFragment,
    receipt: WebCommonsReceipt,
) -> RustyWebResult<WebCommonsIngestPlan> {
    if fragment.protocol_version != WEB_COMMONS_PROTOCOL_VERSION {
        return Err(RustyWebError::InvalidFragment {
            reason: format!("unsupported protocol_version {}", fragment.protocol_version),
        });
    }
    if fragment.pages.is_empty() {
        return Err(RustyWebError::InvalidFragment {
            reason: "fragment requires at least one page".to_string(),
        });
    }

    let disposition_by_page = receipt
        .dispositions
        .iter()
        .map(|disposition| (disposition.page_id.clone(), disposition))
        .collect::<BTreeMap<_, _>>();
    let accepted_pages = disposition_by_page
        .iter()
        .filter_map(|(page_id, disposition)| {
            (disposition.disposition != "dropped").then(|| page_id.clone())
        })
        .collect::<BTreeSet<_>>();
    let page_by_id = fragment
        .pages
        .iter()
        .map(|page| (page.id.clone(), page))
        .collect::<BTreeMap<_, _>>();
    let license_by_domain = fragment
        .source_licenses
        .iter()
        .map(|license| (license.domain.clone(), license))
        .collect::<BTreeMap<_, _>>();
    let mut mutations = Vec::new();
    let peer_node_id = web_commons_peer_id(&fragment.peer_id);

    mutations.push(GraphMutation::NodeUpsert(NodeRecord::new(
        peer_node_id.clone(),
        [LABEL_WEB_COMMONS_PEER],
        json!({
            "peer_id": fragment.peer_id,
            "trust_context": "web_commons",
            "trust_tier": "unknown",
            "trust_weight": 0.3,
            "last_graph_delta_hash": fragment.graph_delta_hash,
        }),
    )));

    for domain in &fragment.domains {
        let accepted_domain_pages = domain
            .page_ids
            .iter()
            .filter(|page_id| accepted_pages.contains(*page_id))
            .cloned()
            .collect::<Vec<_>>();
        if accepted_domain_pages.is_empty() {
            continue;
        }
        mutations.push(GraphMutation::NodeUpsert(NodeRecord::new(
            domain_id(&domain.domain),
            [LABEL_DOMAIN],
            json!({
                "domain": domain.domain,
                "page_ids": accepted_domain_pages,
                "namespace": "web_commons",
                "source_graph": "web_commons",
                "federable": true,
            }),
        )));
    }

    for page in &fragment.pages {
        if !accepted_pages.contains(&page.id) {
            continue;
        }
        let disposition =
            disposition_by_page
                .get(&page.id)
                .ok_or_else(|| RustyWebError::InvalidFragment {
                    reason: format!("missing disposition for page {}", page.id),
                })?;
        let license = license_by_domain.get(&page.domain);
        mutations.push(GraphMutation::NodeUpsert(NodeRecord::new(
            page.id.clone(),
            [LABEL_PAGE],
            json!({
                "url": page.url,
                "domain": page.domain,
                "title": page.title,
                "status": page.status,
                "fetched_at": page.fetched_at,
                "source_class": page.source_class,
                "namespace": "web_commons",
                "source_graph": license
                    .map(|license| license.source_graph.as_str())
                    .unwrap_or("web_commons"),
                "source_license": license
                    .map(|license| license.source_license.as_str())
                    .unwrap_or("unknown"),
                "federable": true,
                "page_state": "federated",
                "admission_tier": disposition.disposition,
                "admission_reason": disposition.reason,
                "peer_id": fragment.peer_id,
                "graph_delta_hash": fragment.graph_delta_hash,
            }),
        )));
        mutations.push(GraphMutation::NodeUpsert(NodeRecord::new(
            web_commons_attestation_id(&fragment.peer_id, &fragment.graph_delta_hash, &page.id),
            [LABEL_WEB_COMMONS_ATTESTATION],
            json!({
                "peer_id": fragment.peer_id,
                "page_id": page.id,
                "domain": page.domain,
                "source_class": page.source_class,
                "fetched_at": page.fetched_at,
                "graph_delta_hash": fragment.graph_delta_hash,
                "admission_tier": disposition.disposition,
            }),
        )));
        let attestation_id =
            web_commons_attestation_id(&fragment.peer_id, &fragment.graph_delta_hash, &page.id);
        mutations.push(GraphMutation::EdgeUpsert(EdgeRecord::new(
            edge_id(&attestation_id, EDGE_SUBMITTED_BY, &peer_node_id),
            attestation_id.clone(),
            EDGE_SUBMITTED_BY,
            peer_node_id.clone(),
            json!({"trust_context": "web_commons"}),
        )));
        mutations.push(GraphMutation::EdgeUpsert(EdgeRecord::new(
            edge_id(&attestation_id, EDGE_ATTESTS_PAGE, &page.id),
            attestation_id,
            EDGE_ATTESTS_PAGE,
            page.id.clone(),
            json!({"graph_delta_hash": fragment.graph_delta_hash}),
        )));
    }

    for snapshot in &fragment.snapshots {
        if !accepted_pages.contains(&snapshot.page_id) {
            continue;
        }
        mutations.push(GraphMutation::NodeUpsert(NodeRecord::new(
            snapshot.id.clone(),
            [LABEL_CONTENT_SNAPSHOT],
            json!({
                "content_hash": snapshot.content_hash,
                "hash_algorithm": "blake3",
                "text": snapshot.text,
                "byte_len": snapshot.text.len(),
                "content_type": snapshot.content_type,
                "namespace": "web_commons",
                "source_graph": "web_commons",
                "federable": true,
                "peer_id": fragment.peer_id,
                "graph_delta_hash": fragment.graph_delta_hash,
            }),
        )));
        mutations.push(GraphMutation::EdgeUpsert(EdgeRecord::new(
            edge_id(&snapshot.page_id, EDGE_HAS_SNAPSHOT, &snapshot.id),
            snapshot.page_id.clone(),
            EDGE_HAS_SNAPSHOT,
            snapshot.id.clone(),
            json!({"source": "web_commons"}),
        )));
    }

    for link in &fragment.edges {
        if !accepted_pages.contains(&link.from_page_id)
            || !accepted_pages.contains(&link.to_page_id)
        {
            continue;
        }
        let admission_tier = match (
            disposition_by_page.get(&link.from_page_id),
            disposition_by_page.get(&link.to_page_id),
        ) {
            (Some(from), Some(to))
                if from.disposition == "canonical" && to.disposition == "canonical" =>
            {
                "canonical"
            }
            _ => "probationary",
        };
        mutations.push(GraphMutation::EdgeUpsert(EdgeRecord::new(
            edge_id(&link.from_page_id, EDGE_LINKS_TO, &link.to_page_id),
            link.from_page_id.clone(),
            EDGE_LINKS_TO,
            link.to_page_id.clone(),
            json!({
                "source": "web_commons",
                "anchor": link.anchor,
                "admission_tier": admission_tier,
                "peer_id": fragment.peer_id,
                "graph_delta_hash": fragment.graph_delta_hash,
            }),
        )));

        if let Some(page) = page_by_id.get(&link.to_page_id) {
            mutations.push(GraphMutation::EdgeUpsert(EdgeRecord::new(
                edge_id(&link.to_page_id, EDGE_ON_DOMAIN, &domain_id(&page.domain)),
                link.to_page_id.clone(),
                EDGE_ON_DOMAIN,
                domain_id(&page.domain),
                json!({"source": "web_commons"}),
            )));
        }
    }

    Ok(WebCommonsIngestPlan {
        batch: GraphMutationBatch { mutations },
        receipt,
    })
}

pub async fn fetch_seed_pages(
    request: &CrawlRequest,
    options: &LiveFetchOptions,
) -> RustyWebResult<Vec<FetchedPage>> {
    let seeds = validate_crawl_request(request, &options.guard_policy)?;
    let seed_domains = seeds
        .iter()
        .filter_map(|seed| domain_for_url(seed).ok())
        .collect::<BTreeSet<_>>();
    let mut effective_options = options.clone();
    effective_options.timeout_seconds = options
        .timeout_seconds
        .min(request.budget.max_seconds)
        .max(1);
    let fetcher = FetchCascade::new(FetchCascadeOptions {
        user_agent: effective_options.user_agent.clone(),
        timeout_seconds: effective_options.timeout_seconds,
    })?;
    let mut pages = Vec::new();
    let mut remaining_bytes = request.budget.max_bytes;
    let mut last_fetch_by_domain: BTreeMap<String, Instant> = BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(request.budget.max_seconds);
    let mut seen = BTreeSet::new();
    let mut frontier = VecDeque::new();

    for seed in seeds {
        if seen.insert(seed.clone()) {
            frontier.push_back((seed, 0usize));
        }
    }

    while let Some((url, depth)) = frontier.pop_front() {
        if remaining_bytes == 0 || pages.len() >= request.budget.max_pages {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }

        if effective_options.respect_robots {
            let decision = global_robots_cache()
                .check(fetcher.client(), &url, &effective_options.user_agent)
                .await?;
            if !decision.allowed {
                if depth == 0 {
                    return Err(RustyWebError::BlockedUrl {
                        url,
                        reason: format!("robots disallowed: {}", decision.reason),
                    });
                }
                continue;
            }
            honor_crawl_delay(&url, &decision, &mut last_fetch_by_domain).await?;
        }

        let result = fetcher.fetch_with_promotion(&url, remaining_bytes).await?;
        let final_url =
            guarded_canonicalize_url(&result.final_url, &effective_options.guard_policy)?;
        let page = FetchedPage {
            url: final_url,
            status: result.http_status,
            body: String::from_utf8_lossy(&result.html_bytes).into_owned(),
            content_type: result.content_type,
            fetched_at: current_unix_ms_string(),
        };
        remaining_bytes = remaining_bytes.saturating_sub(page.body.len());
        if depth < request.budget.max_depth
            && (200..400).contains(&page.status)
            && page.content_type.to_ascii_lowercase().contains("html")
        {
            for target in extract_links_for_url(&page.url, &page.body)? {
                let Ok(target) = guarded_canonicalize_url(&target, &effective_options.guard_policy)
                else {
                    continue;
                };
                if !request.scope.follow_offsite
                    && !target_stays_on_seed_domains(&seed_domains, &target)
                {
                    continue;
                }
                if seen.insert(target.clone()) {
                    frontier.push_back((target, depth + 1));
                }
            }
        }
        pages.push(page);
    }

    Ok(pages)
}

fn target_stays_on_seed_domains(seed_domains: &BTreeSet<String>, target_url: &str) -> bool {
    domain_for_url(target_url)
        .map(|domain| seed_domains.contains(&domain))
        .unwrap_or(false)
}

pub fn apply_batch_to_store(
    store: &mut impl GraphStore,
    batch: &GraphMutationBatch,
) -> GraphStoreResult<Vec<GraphWriteResult>> {
    let mut writes = Vec::with_capacity(batch.mutations.len());
    for mutation in &batch.mutations {
        match mutation {
            GraphMutation::NodeUpsert(node) => writes.push(store.upsert_node(node.clone())?),
            GraphMutation::EdgeUpsert(edge) => writes.push(store.upsert_edge(edge.clone())?),
        }
    }
    Ok(writes)
}

pub fn validate_crawl_request(
    request: &CrawlRequest,
    policy: &UrlGuardPolicy,
) -> RustyWebResult<Vec<String>> {
    if request.seeds.is_empty() {
        return Err(RustyWebError::EmptySeeds);
    }
    if request.budget.max_pages == 0 {
        return Err(RustyWebError::InvalidBudget {
            reason: "max_pages must be greater than zero".to_string(),
        });
    }
    if request.budget.max_seconds == 0 {
        return Err(RustyWebError::InvalidBudget {
            reason: "max_seconds must be greater than zero".to_string(),
        });
    }
    if request.budget.max_bytes == 0 {
        return Err(RustyWebError::InvalidBudget {
            reason: "max_bytes must be greater than zero".to_string(),
        });
    }

    let mut seeds = BTreeSet::new();
    for seed in &request.seeds {
        seeds.insert(guarded_canonicalize_url(seed, policy)?);
    }
    Ok(seeds.into_iter().collect())
}

pub fn canonicalize_url(raw: &str) -> RustyWebResult<String> {
    let mut url = Url::parse(raw).map_err(|err| RustyWebError::InvalidUrl {
        url: raw.to_string(),
        reason: err.to_string(),
    })?;
    url.set_fragment(None);
    url.set_username("").ok();
    url.set_password(None).ok();
    if matches!(url.scheme(), "http" | "https") {
        Ok(url.to_string())
    } else {
        Err(RustyWebError::InvalidUrl {
            url: raw.to_string(),
            reason: format!("unsupported scheme {}", url.scheme()),
        })
    }
}

pub fn guarded_canonicalize_url(raw: &str, policy: &UrlGuardPolicy) -> RustyWebResult<String> {
    let canonical = canonicalize_url(raw)?;
    guard_canonical_url(&canonical, policy)?;
    Ok(canonical)
}

pub fn guard_canonical_url(canonical: &str, policy: &UrlGuardPolicy) -> RustyWebResult<()> {
    let url = Url::parse(canonical).map_err(|err| RustyWebError::InvalidUrl {
        url: canonical.to_string(),
        reason: err.to_string(),
    })?;
    let host =
        url.host_str()
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| RustyWebError::InvalidUrl {
                url: canonical.to_string(),
                reason: "missing host".to_string(),
            })?;

    if policy.block_metadata_services && is_metadata_service_host(&host) {
        return Err(RustyWebError::BlockedUrl {
            url: canonical.to_string(),
            reason: "metadata service host".to_string(),
        });
    }
    if host == "localhost" && !policy.allow_loopback {
        return Err(RustyWebError::BlockedUrl {
            url: canonical.to_string(),
            reason: "loopback hostname".to_string(),
        });
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if let Some(reason) = blocked_ip_reason(ip, policy) {
            return Err(RustyWebError::BlockedUrl {
                url: canonical.to_string(),
                reason: reason.to_string(),
            });
        }
    }

    Ok(())
}

pub fn extract_links(base_url: &str, html: &str) -> RustyWebResult<Vec<String>> {
    extract_links_with_profile(base_url, html, &ExtractionProfile::default())
}

pub fn extract_links_for_url(base_url: &str, html: &str) -> RustyWebResult<Vec<String>> {
    let base = Url::parse(base_url).map_err(|err| RustyWebError::InvalidUrl {
        url: base_url.to_string(),
        reason: err.to_string(),
    })?;
    extract_links_with_profile(base_url, html, &profile_for_url(&base))
}

pub fn extract_links_with_profile(
    base_url: &str,
    html: &str,
    profile: &ExtractionProfile,
) -> RustyWebResult<Vec<String>> {
    if !profile.include_links {
        return Ok(Vec::new());
    }
    let base = Url::parse(base_url).map_err(|err| RustyWebError::InvalidUrl {
        url: base_url.to_string(),
        reason: err.to_string(),
    })?;
    let links = Rc::new(RefCell::new(BTreeSet::new()));
    let links_for_handler = Rc::clone(&links);
    let mut rewriter = HtmlRewriter::new(
        Settings {
            element_content_handlers: vec![element!("a[href]", move |el| {
                let Some(raw_href) = el.get_attribute("href") else {
                    return Ok(());
                };
                if let Ok(mut joined) = base.join(&raw_href) {
                    joined.set_fragment(None);
                    if matches!(joined.scheme(), "http" | "https") {
                        links_for_handler.borrow_mut().insert(joined.to_string());
                    }
                }
                Ok(())
            })],
            ..Settings::default()
        },
        |_chunk: &[u8]| {},
    );
    rewriter
        .write(html.as_bytes())
        .map_err(|err| RustyWebError::HtmlParse {
            reason: err.to_string(),
        })?;
    rewriter.end().map_err(|err| RustyWebError::HtmlParse {
        reason: err.to_string(),
    })?;
    let extracted = links.borrow().iter().cloned().collect();
    Ok(extracted)
}

pub fn graph_delta_hash(batch: &GraphMutationBatch) -> String {
    let bytes = serde_json::to_vec(batch).unwrap_or_default();
    blake3_hash(&bytes)
}

fn select_budgeted_pages(budget: &CrawlBudget, pages: &[FetchedPage]) -> (Vec<FetchedPage>, bool) {
    let mut selected = Vec::new();
    let mut consumed_bytes = 0usize;
    let mut budget_limited = pages.len() > budget.max_pages;

    for page in pages.iter().take(budget.max_pages) {
        let next_bytes = page.body.len();
        if consumed_bytes.saturating_add(next_bytes) > budget.max_bytes {
            budget_limited = true;
            break;
        }
        consumed_bytes += next_bytes;
        selected.push(page.clone());
    }

    (selected, budget_limited)
}

fn add_seed_nodes(graph: &mut CrawlGraph, request: &CrawlRequest, seeds: &[String]) {
    let run_node_id = crawl_run_id(&request.run_id);
    for seed in seeds {
        let seed_id = discovery_seed_id(seed);
        graph
            .batch
            .mutations
            .push(GraphMutation::NodeUpsert(NodeRecord::new(
                seed_id.clone(),
                [LABEL_DISCOVERY_SEED],
                json!({
                    "url": seed,
                    "namespace": request.scope.namespace,
                    "seed_hash": stable_part(seed),
                }),
            )));
        graph
            .batch
            .mutations
            .push(GraphMutation::EdgeUpsert(EdgeRecord::new(
                edge_id(&run_node_id, EDGE_SEEDED, &seed_id),
                run_node_id.clone(),
                EDGE_SEEDED,
                seed_id,
                json!({"source": "crawl_request"}),
            )));
    }
}

fn add_receipt_node(graph: &mut CrawlGraph, receipt: &CrawlReceipt) {
    let run_node_id = crawl_run_id(&receipt.run_id);
    let mut properties = serde_json::to_value(receipt).unwrap_or_else(|_| {
        json!({
            "receipt_id": receipt.receipt_id,
            "run_id": receipt.run_id,
            "status": receipt.status,
        })
    });
    let props = properties_object(&mut properties);
    insert_scope_properties(props, &receipt.scope);
    props.insert(
        "graph_delta_hash_algorithm".to_string(),
        Value::String("blake3".to_string()),
    );

    graph
        .batch
        .mutations
        .push(GraphMutation::NodeUpsert(NodeRecord::new(
            receipt.receipt_id.clone(),
            [LABEL_CRAWL_RECEIPT],
            properties,
        )));
    graph.batch.mutations.push(GraphMutation::EdgeUpsert(
        EdgeRecord::new(
            edge_id(&run_node_id, EDGE_EMITTED_RECEIPT, &receipt.receipt_id),
            run_node_id,
            EDGE_EMITTED_RECEIPT,
            receipt.receipt_id.clone(),
            json!({
                "receipt_id": receipt.receipt_id,
                "graph_delta_hash": receipt.graph_delta_hash,
            }),
        )
        .with_provenance(provenance_for_scope(&receipt.scope)),
    ));
}

fn annotate_graph_for_scope(graph: &mut CrawlGraph, scope: &CrawlScope) {
    for mutation in &mut graph.batch.mutations {
        match mutation {
            GraphMutation::NodeUpsert(node) => {
                let props = properties_object(&mut node.properties);
                insert_scope_properties(props, scope);
            }
            GraphMutation::EdgeUpsert(edge) => {
                let props = properties_object(&mut edge.properties);
                insert_scope_properties(props, scope);
                edge.provenance = Some(provenance_for_scope(scope));
            }
        }
    }
}

fn properties_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("object inserted above")
}

fn insert_scope_properties(properties: &mut Map<String, Value>, scope: &CrawlScope) {
    properties.insert(
        "source_graph".to_string(),
        Value::String(scope.source_graph.clone()),
    );
    properties.insert(
        "source_license".to_string(),
        Value::String(scope.source_license.clone()),
    );
    properties.insert("federable".to_string(), Value::Bool(scope.federable));
    properties.insert(
        "admission_tier".to_string(),
        Value::String("advisory".to_string()),
    );
    properties.insert(
        "follow_offsite".to_string(),
        Value::Bool(scope.follow_offsite),
    );
    if let Some(expires_at_ms) = scope.ttl_expires_at_ms {
        properties.insert(TTL_PROPERTY.to_string(), json!(expires_at_ms));
    }
    if !scope.actor_id.is_empty() {
        properties.insert(
            "actor_id".to_string(),
            Value::String(scope.actor_id.clone()),
        );
    }
}

fn property_string(properties: &Value, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

fn property_u16(properties: &Value, key: &str) -> Option<u16> {
    properties
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
}

fn bounded_text(text: String, max_bytes: usize) -> String {
    if max_bytes == 0 || text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn provenance_for_scope(scope: &CrawlScope) -> Provenance {
    Provenance {
        source_id: Some(scope.source_graph.clone()),
        timestamp: None,
        method: Some("rustyweb_v2".to_string()),
    }
}

fn is_metadata_service_host(host: &str) -> bool {
    matches!(
        host,
        "169.254.169.254" | "169.254.170.2" | "metadata" | "metadata.google.internal"
    )
}

fn blocked_ip_reason(ip: IpAddr, policy: &UrlGuardPolicy) -> Option<&'static str> {
    match ip {
        IpAddr::V4(ip) => {
            if policy.block_metadata_services
                && (ip.octets() == [169, 254, 169, 254] || ip.octets() == [169, 254, 170, 2])
            {
                return Some("metadata service ip address");
            }
            if ip.is_loopback() && !policy.allow_loopback {
                return Some("loopback ip address");
            }
            if (ip.is_private() || ip.is_link_local() || ip.is_unspecified())
                && !policy.allow_private_networks
            {
                return Some("private, link-local, or unspecified ip address");
            }
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback() && !policy.allow_loopback {
                return Some("loopback ip address");
            }
            if (ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_unspecified())
                && !policy.allow_private_networks
            {
                return Some("private, link-local, or unspecified ip address");
            }
        }
    }
    None
}

fn current_unix_ms_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

async fn honor_crawl_delay(
    url: &str,
    decision: &RobotsDecision,
    last_fetch_by_domain: &mut BTreeMap<String, Instant>,
) -> RustyWebResult<()> {
    let Some(delay) = crawl_delay_duration(decision) else {
        return Ok(());
    };
    let domain = domain_for_url(url)?;
    if let Some(last_fetch) = last_fetch_by_domain.get(&domain) {
        let elapsed = last_fetch.elapsed();
        if elapsed < delay {
            tokio::time::sleep(delay - elapsed).await;
        }
    }
    last_fetch_by_domain.insert(domain, Instant::now());
    Ok(())
}

fn insert_node(nodes: &mut BTreeMap<String, NodeRecord>, node: NodeRecord) {
    nodes.insert(node.id.clone(), node);
}

fn insert_domain_node(
    nodes: &mut BTreeMap<String, NodeRecord>,
    domain: &str,
    id: &str,
    namespace: &str,
) {
    insert_node(
        nodes,
        NodeRecord::new(
            id.to_string(),
            [LABEL_DOMAIN],
            json!({"domain": domain, "namespace": namespace}),
        ),
    );
}

fn insert_robots_node(
    nodes: &mut BTreeMap<String, NodeRecord>,
    domain: &str,
    id: &str,
    namespace: &str,
) {
    insert_node(
        nodes,
        NodeRecord::new(
            id.to_string(),
            [LABEL_ROBOTS_POLICY],
            json!({
                "domain": domain,
                "namespace": namespace,
                "policy_state": "fixture_assumed_allowed",
            }),
        ),
    );
}

fn insert_page_node(
    nodes: &mut BTreeMap<String, NodeRecord>,
    url: &str,
    id: &str,
    domain: &str,
    namespace: &str,
    page_state: &str,
) {
    nodes.entry(id.to_string()).or_insert_with(|| {
        NodeRecord::new(
            id.to_string(),
            [LABEL_PAGE],
            json!({
                "url": url,
                "domain": domain,
                "namespace": namespace,
                "page_state": page_state,
                "source_class": Url::parse(url)
                    .ok()
                    .map(|parsed| classify_url(&parsed).as_str())
                    .unwrap_or(SourceClass::Unknown.as_str()),
            }),
        )
    });
}

fn insert_edge(
    edges: &mut BTreeMap<String, EdgeRecord>,
    from_id: &str,
    edge_type: &str,
    to_id: &str,
    properties: Value,
) -> bool {
    let id = edge_id(from_id, edge_type, to_id);
    let inserted = !edges.contains_key(&id);
    edges.insert(
        id.clone(),
        EdgeRecord::new(id, from_id, edge_type, to_id, properties),
    );
    inserted
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

fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn crawl_run_id(run_id: &str) -> String {
    format!("crawl_run:{}", stable_part(run_id))
}

fn fetch_attempt_id(run_id: &str, index: usize, url: &str) -> String {
    format!(
        "fetch_attempt:{}",
        stable_part(&format!("{run_id}:{index}:{url}"))
    )
}

fn domain_id(domain: &str) -> String {
    format!("domain:{domain}")
}

fn page_id(url: &str) -> String {
    format!("page:{}", stable_part(url))
}

fn page_alias_id(url: &str) -> String {
    format!("page_alias:{}", stable_part(url))
}

fn content_snapshot_id(content_hash: &str) -> String {
    format!("content_snapshot:{content_hash}")
}

fn robots_policy_id(domain: &str) -> String {
    format!("robots_policy:{}", stable_part(domain))
}

fn discovery_seed_id(seed: &str) -> String {
    format!("discovery_seed:{}", stable_part(seed))
}

fn web_commons_peer_id(peer_id: &str) -> String {
    format!("web_commons_peer:{}", stable_part(peer_id))
}

fn web_commons_attestation_id(peer_id: &str, graph_delta_hash: &str, page_id: &str) -> String {
    format!(
        "web_commons_attestation:{}",
        stable_part(&format!("{peer_id}:{graph_delta_hash}:{page_id}"))
    )
}

fn crawl_receipt_id(run_id: &str, graph_delta_hash: &str) -> String {
    format!(
        "crawl_receipt:{}",
        stable_part(&format!("{run_id}:{graph_delta_hash}"))
    )
}

fn edge_id(from_id: &str, edge_type: &str, to_id: &str) -> String {
    format!(
        "edge:{}",
        stable_part(&format!("{from_id}:{edge_type}:{to_id}"))
    )
}

fn stable_part(value: &str) -> String {
    blake3_hash(value.as_bytes())[..24].to_string()
}

fn blake3_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

impl From<RustyWebError> for GraphStoreError {
    fn from(err: RustyWebError) -> Self {
        GraphStoreError::new("rustyweb_error", err.to_string())
    }
}
