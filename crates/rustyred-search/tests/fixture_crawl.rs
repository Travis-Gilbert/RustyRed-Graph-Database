use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use rustyred_core::{InMemoryGraphStore, NodeQuery};
use rustyred_search::TTL_PROPERTY;
use rustyred_search::{
    build_fixture_crawl_graph, build_v2_fixture_crawl, build_web_commons_fragment,
    canonicalize_url, extract_links, extract_links_with_profile, guarded_canonicalize_url,
    profile_for, run_live_crawl_with_options, CrawlBudget, CrawlConfig, CrawlRequest, CrawlScope,
    FixturePage, LiveFetchOptions, RustyWebError, SourceClass, UrlGuardPolicy,
    WebCommonsFragmentOptions, EDGE_CANONICAL_OF, EDGE_EMITTED_RECEIPT, EDGE_HAS_SNAPSHOT,
    EDGE_LINKS_TO, EDGE_ON_DOMAIN, EDGE_RESULTED_IN, EDGE_SEEDED, LABEL_CONTENT_SNAPSHOT,
    LABEL_CRAWL_RECEIPT, LABEL_CRAWL_RUN, LABEL_DISCOVERY_SEED, LABEL_DOMAIN, LABEL_FETCH_ATTEMPT,
    LABEL_PAGE, LABEL_ROBOTS_POLICY,
};
use serde_json::json;

fn label_count(graph: &rustyred_search::CrawlGraph, label: &str) -> usize {
    graph
        .nodes()
        .iter()
        .filter(|node| node.labels.iter().any(|candidate| candidate == label))
        .count()
}

fn edge_count(graph: &rustyred_search::CrawlGraph, edge_type: &str) -> usize {
    graph
        .edges()
        .iter()
        .filter(|edge| edge.edge_type == edge_type)
        .count()
}

#[test]
fn fixture_crawl_emits_v0_graph_contract_and_applies_to_store() {
    let graph = build_fixture_crawl_graph(
        CrawlConfig {
            run_id: "rw-fixture-1".to_string(),
            namespace: "link".to_string(),
            user_agent: "RustyWeb test".to_string(),
        },
        &[
            FixturePage::html(
                "https://example.com/index.html#top",
                r#"
                <html>
                  <head><title>Home</title></head>
                  <body>
                    <a href="/about">About</a>
                    <a href="https://docs.example.org/start#section">Docs</a>
                  </body>
                </html>
                "#,
            ),
            FixturePage::html(
                "https://example.com/about",
                r#"<html><body><a href="/index.html">Home</a></body></html>"#,
            ),
        ],
    )
    .unwrap();

    assert_eq!(graph.counters.fetched_pages, 2);
    assert_eq!(graph.counters.discovered_pages, 4);
    assert_eq!(graph.counters.domains, 2);
    assert_eq!(graph.counters.snapshots, 2);
    assert_eq!(graph.counters.links, 3);

    assert_eq!(label_count(&graph, LABEL_CRAWL_RUN), 1);
    assert_eq!(label_count(&graph, LABEL_FETCH_ATTEMPT), 2);
    assert_eq!(label_count(&graph, LABEL_DOMAIN), 2);
    assert_eq!(label_count(&graph, LABEL_PAGE), 4);
    assert_eq!(label_count(&graph, LABEL_CONTENT_SNAPSHOT), 2);
    assert_eq!(label_count(&graph, LABEL_ROBOTS_POLICY), 2);

    assert_eq!(edge_count(&graph, EDGE_RESULTED_IN), 2);
    assert_eq!(edge_count(&graph, EDGE_HAS_SNAPSHOT), 2);
    assert_eq!(edge_count(&graph, EDGE_ON_DOMAIN), 3);
    assert_eq!(edge_count(&graph, EDGE_LINKS_TO), 3);
    assert_eq!(edge_count(&graph, EDGE_CANONICAL_OF), 1);

    let mut store = InMemoryGraphStore::new();
    let writes = graph.apply_to_store(&mut store).unwrap();
    assert_eq!(writes.len(), graph.batch.mutations.len());
    assert_eq!(store.query_nodes(NodeQuery::label(LABEL_PAGE)).len(), 4,);
}

#[test]
fn canonicalization_and_link_extraction_are_deterministic() {
    assert_eq!(
        canonicalize_url("https://example.com/a?b=1#frag").unwrap(),
        "https://example.com/a?b=1"
    );

    let links = extract_links(
        "https://example.com/docs/index.html",
        r#"<a href="../a#x">A</a><a href="mailto:nope@example.com">Mail</a><a href="https://example.org/z">Z</a>"#,
    )
    .unwrap();
    assert_eq!(
        links,
        vec![
            "https://example.com/a".to_string(),
            "https://example.org/z".to_string(),
        ],
    );
}

#[test]
fn extraction_profile_can_disable_html_link_extraction() {
    let links = extract_links_with_profile(
        "https://example.com/file.pdf",
        r#"<a href="https://example.com/nope">Nope</a>"#,
        &profile_for(SourceClass::Pdf),
    )
    .unwrap();
    assert!(links.is_empty());
}

#[test]
fn v2_fixture_crawl_emits_receipt_ttl_license_and_seed_nodes() {
    let mut request = CrawlRequest::new(
        "rw-v2-1",
        vec![
            "https://example.com/index.html#top".to_string(),
            "https://docs.example.org/start".to_string(),
        ],
    );
    request.budget = CrawlBudget {
        max_pages: 1,
        max_seconds: 10,
        max_depth: 1,
        max_bytes: 8192,
    };
    request.scope = CrawlScope {
        namespace: "link".to_string(),
        follow_offsite: true,
        ttl_expires_at_ms: Some(1_893_456_000_000),
        source_graph: "fixture_graph".to_string(),
        source_license: "CC0".to_string(),
        federable: true,
        actor_id: "codex".to_string(),
    };

    let output = build_v2_fixture_crawl(
        request,
        &[
            FixturePage::html(
                "https://example.com/index.html#top",
                r#"
                <html>
                  <body>
                    <a href="/about">About</a>
                    <a href="https://docs.example.org/start">Docs</a>
                  </body>
                </html>
                "#,
            ),
            FixturePage::html(
                "https://example.com/about",
                r#"<html><body><a href="/index.html">Home</a></body></html>"#,
            ),
        ],
    )
    .unwrap();

    assert_eq!(output.receipt.status, "budget_limited");
    assert_eq!(output.receipt.seed_count, 2);
    assert_eq!(output.receipt.counters.fetched_pages, 1);
    assert!(!output.receipt.graph_delta_hash.is_empty());

    assert_eq!(label_count(&output.graph, LABEL_CRAWL_RECEIPT), 1);
    assert_eq!(label_count(&output.graph, LABEL_DISCOVERY_SEED), 2);
    assert_eq!(edge_count(&output.graph, EDGE_EMITTED_RECEIPT), 1);
    assert_eq!(edge_count(&output.graph, EDGE_SEEDED), 2);

    let fetched_page = output
        .graph
        .nodes()
        .into_iter()
        .find(|node| {
            node.labels.iter().any(|label| label == LABEL_PAGE)
                && node.properties.get("page_state") == Some(&json!("fetched"))
        })
        .unwrap();
    assert_eq!(
        fetched_page.properties.get(TTL_PROPERTY),
        Some(&json!(1_893_456_000_000i64))
    );
    assert_eq!(
        fetched_page.properties.get("source_graph"),
        Some(&json!("fixture_graph"))
    );
    assert_eq!(
        fetched_page.properties.get("source_license"),
        Some(&json!("CC0"))
    );
    assert_eq!(fetched_page.properties.get("federable"), Some(&json!(true)));
    assert_eq!(
        fetched_page.properties.get("admission_tier"),
        Some(&json!("advisory"))
    );

    let link_edge = output
        .graph
        .edges()
        .into_iter()
        .find(|edge| edge.edge_type == EDGE_LINKS_TO)
        .unwrap();
    assert_eq!(
        link_edge
            .provenance
            .as_ref()
            .and_then(|p| p.method.as_deref()),
        Some("rustyweb_v2")
    );
    assert_eq!(
        link_edge.properties.get("source_license"),
        Some(&json!("CC0"))
    );
}

#[test]
fn web_commons_fragment_bounds_text_and_strips_provenance_by_default() {
    let mut request = CrawlRequest::new(
        "rw-commons-1",
        vec!["https://example.com/index.html".to_string()],
    );
    request.scope.federable = true;
    request.scope.actor_id = "operator-a".to_string();
    request.scope.source_license = "CC0".to_string();
    let output = build_v2_fixture_crawl(
        request.clone(),
        &[FixturePage::html(
            "https://example.com/index.html",
            "<html><body>alpha beta gamma delta</body></html>",
        )],
    )
    .unwrap();

    let fragment = build_web_commons_fragment(
        &output,
        &request,
        "peer-1",
        &WebCommonsFragmentOptions {
            include_provenance: false,
            snapshot_text_bytes: 10,
        },
    )
    .unwrap();

    assert_eq!(fragment.peer_id, "peer-1");
    assert_eq!(fragment.pages.len(), 1);
    assert_eq!(fragment.snapshots.len(), 1);
    assert_eq!(fragment.source_licenses[0].source_license, "CC0");
    assert!(fragment.provenance.is_none());
    assert!(fragment.snapshots[0].text.len() <= 10);
    assert!(!fragment.signing_bytes().unwrap().is_empty());
}

#[test]
fn url_guard_blocks_metadata_loopback_and_private_ips() {
    let policy = UrlGuardPolicy::default();

    assert!(guarded_canonicalize_url("http://169.254.169.254/latest/meta-data/", &policy).is_err());
    assert!(guarded_canonicalize_url("http://127.0.0.1:8000/", &policy).is_err());
    assert!(guarded_canonicalize_url("http://10.0.0.1/", &policy).is_err());
    assert_eq!(
        guarded_canonicalize_url("https://example.com/a#b", &policy).unwrap(),
        "https://example.com/a"
    );
}

#[tokio::test]
async fn live_fetch_loop_feeds_the_v2_receipt_contract() {
    let url = spawn_one_shot_server(
        200,
        "text/html; charset=utf-8",
        r#"<html><body><h1>Live fixture</h1><a href="/next">Next</a></body></html>"#,
    );
    let mut request = CrawlRequest::new("rw-live-1", vec![url]);
    request.budget = CrawlBudget {
        max_pages: 1,
        max_seconds: 5,
        max_depth: 0,
        max_bytes: 8192,
    };
    request.scope = CrawlScope {
        namespace: "link".to_string(),
        follow_offsite: false,
        ttl_expires_at_ms: Some(1_893_456_000_000),
        source_graph: "live_fixture".to_string(),
        source_license: "local-test".to_string(),
        federable: false,
        actor_id: "codex".to_string(),
    };
    let options = live_test_options();

    let output = run_live_crawl_with_options(request, &options)
        .await
        .unwrap();

    assert_eq!(output.receipt.status, "completed");
    assert_eq!(output.receipt.counters.fetched_pages, 1);
    assert_eq!(output.receipt.counters.links, 1);
    assert_eq!(label_count(&output.graph, LABEL_CRAWL_RECEIPT), 1);
    assert_eq!(label_count(&output.graph, LABEL_CONTENT_SNAPSHOT), 1);

    let snapshot = output
        .graph
        .nodes()
        .into_iter()
        .find(|node| {
            node.labels
                .iter()
                .any(|label| label == LABEL_CONTENT_SNAPSHOT)
        })
        .unwrap();
    assert_eq!(
        snapshot.properties.get("source_graph"),
        Some(&json!("live_fixture"))
    );
    assert!(snapshot
        .properties
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap()
        .contains("Live fixture"));
}

#[tokio::test]
async fn live_frontier_fetches_discovered_links_within_depth() {
    let url = spawn_path_server(vec![
        (
            "/",
            200,
            "text/html; charset=utf-8",
            r#"<html><body><h1>Root</h1><a href="/next">Next</a></body></html>"#,
        ),
        (
            "/next",
            200,
            "text/html; charset=utf-8",
            r#"<html><body><h1>Second frontier page</h1></body></html>"#,
        ),
    ]);
    let mut request = CrawlRequest::new("rw-live-frontier", vec![url]);
    request.budget = CrawlBudget {
        max_pages: 2,
        max_seconds: 5,
        max_depth: 1,
        max_bytes: 16_384,
    };
    request.scope.follow_offsite = false;

    let output = run_live_crawl_with_options(request, &live_test_options())
        .await
        .unwrap();

    assert_eq!(output.receipt.status, "completed");
    assert_eq!(output.receipt.counters.fetched_pages, 2);
    assert_eq!(output.receipt.counters.snapshots, 2);
    assert_eq!(output.receipt.counters.links, 1);
    assert!(output.graph.nodes().into_iter().any(|node| {
        node.labels
            .iter()
            .any(|label| label == LABEL_CONTENT_SNAPSHOT)
            && node
                .properties
                .get("text")
                .and_then(|value| value.as_str())
                .is_some_and(|text| text.contains("Second frontier page"))
    }));
}

#[tokio::test]
async fn live_fetch_loop_enforces_body_budget() {
    let url = spawn_one_shot_server(200, "text/plain", "0123456789");
    let mut request = CrawlRequest::new("rw-live-cap", vec![url]);
    request.budget = CrawlBudget {
        max_pages: 1,
        max_seconds: 5,
        max_depth: 0,
        max_bytes: 4,
    };

    let error = run_live_crawl_with_options(request, &live_test_options())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        RustyWebError::BodyLimitExceeded { limit: 4, .. }
    ));
}

#[tokio::test]
async fn live_fetch_loop_fetches_robots_before_page() {
    let url = spawn_sequence_server(vec![
        (
            200,
            "text/plain",
            "User-agent: *\nAllow: /\nCrawl-delay: 0\n",
        ),
        (
            200,
            "text/html; charset=utf-8",
            r#"<html><body><h1>Robots allowed</h1><a href="/next">Next</a></body></html>"#,
        ),
    ]);
    let mut request = CrawlRequest::new("rw-live-robots-allow", vec![url]);
    request.budget = CrawlBudget {
        max_pages: 1,
        max_seconds: 5,
        max_depth: 0,
        max_bytes: 8192,
    };

    let output = run_live_crawl_with_options(request, &live_robots_options(true))
        .await
        .unwrap();

    assert_eq!(output.receipt.counters.fetched_pages, 1);
    assert_eq!(output.receipt.counters.links, 1);
}

#[tokio::test]
async fn live_fetch_loop_blocks_robots_disallow() {
    let url = spawn_sequence_server(vec![(200, "text/plain", "User-agent: *\nDisallow: /\n")]);
    let mut request = CrawlRequest::new("rw-live-robots-deny", vec![url]);
    request.budget = CrawlBudget {
        max_pages: 1,
        max_seconds: 5,
        max_depth: 0,
        max_bytes: 8192,
    };

    let error = run_live_crawl_with_options(request, &live_robots_options(true))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        RustyWebError::BlockedUrl { reason, .. } if reason.contains("robots disallowed")
    ));
}

#[tokio::test]
async fn live_fetch_loop_skips_discovered_robots_disallow() {
    let url = spawn_path_server(vec![
        (
            "/robots.txt",
            200,
            "text/plain",
            "User-agent: *\nAllow: /\nDisallow: /private\n",
        ),
        (
            "/",
            200,
            "text/html; charset=utf-8",
            r#"<html><body><h1>Public page</h1><a href="/private">Private</a></body></html>"#,
        ),
    ]);
    let mut request = CrawlRequest::new("rw-live-robots-discovered-deny", vec![url]);
    request.budget = CrawlBudget {
        max_pages: 2,
        max_seconds: 5,
        max_depth: 1,
        max_bytes: 8192,
    };
    request.scope.follow_offsite = false;

    let output = run_live_crawl_with_options(request, &live_robots_options(true))
        .await
        .unwrap();

    assert_eq!(output.receipt.counters.fetched_pages, 1);
    assert_eq!(output.receipt.counters.snapshots, 1);
    assert_eq!(output.receipt.counters.links, 1);
}

fn live_test_options() -> LiveFetchOptions {
    live_robots_options(false)
}

fn live_robots_options(respect_robots: bool) -> LiveFetchOptions {
    LiveFetchOptions {
        user_agent: "RustyWeb test".to_string(),
        timeout_seconds: 5,
        guard_policy: UrlGuardPolicy {
            allow_loopback: true,
            allow_private_networks: false,
            block_metadata_services: true,
        },
        respect_robots,
    }
}

fn spawn_one_shot_server(status: u16, content_type: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request_buf = [0; 1024];
        let _ = stream.read(&mut request_buf);
        let response = format!(
            "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });
    format!("http://{address}/")
}

fn spawn_sequence_server(responses: Vec<(u16, &'static str, &'static str)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for (status, content_type, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_buf = [0; 1024];
            let _ = stream.read(&mut request_buf);
            let response = format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{address}/")
}

fn spawn_path_server(routes: Vec<(&'static str, u16, &'static str, &'static str)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for _ in 0..routes.len() {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_buf = [0; 2048];
            let read = stream.read(&mut request_buf).unwrap_or_default();
            let request = String::from_utf8_lossy(&request_buf[..read]);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let route = routes
                .iter()
                .find(|(candidate, _, _, _)| *candidate == path)
                .unwrap_or(&("/", 404, "text/plain", "not found"));
            let (_, status, content_type, body) = *route;
            let reason = if status == 404 { "Not Found" } else { "OK" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{address}/")
}
