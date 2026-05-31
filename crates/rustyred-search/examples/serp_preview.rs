//! Preview harness for the browser SERP.
//!
//! Builds a small fixture substrate (the same `build_v2_fixture_crawl` write path
//! the browser uses), runs the real substrate search over it, renders the SERP,
//! and writes the page to a file so the node-and-edge graph can be opened in a
//! browser for visual verification. This is a dev tool, not a shipped surface:
//! the data is a fixture web, and the SERP it renders is the real renderer that
//! ships and draws real browser-search output in production.
//!
//!   cargo run -p rustyred-search --example serp_preview -- /tmp/rustyweb-serp.html apple

use std::env;
use std::fs;

use rustyred_core::graph_store::InMemoryGraphStore;
use rustyred_search::{
    build_v2_fixture_crawl, render_serp_html, search_substrate, CrawlRequest, FetchedPage,
    SearchOptions,
};

fn page(url: &str, body: &str) -> FetchedPage {
    FetchedPage {
        url: url.to_string(),
        status: 200,
        body: body.to_string(),
        content_type: "text/html; charset=utf-8".to_string(),
        fetched_at: String::new(),
    }
}

fn main() {
    // A small browsed "pomology" web: an apple cluster (interlinked) plus an
    // unrelated tropical cluster, so a search for "apple" returns a shaped
    // neighbourhood, not the whole graph.
    let pages = vec![
        page(
            "http://pomology.example/apple",
            r#"<h1>Apple</h1><p>The apple is a pome fruit; apple cultivation spans many varieties and orchards.</p>
               <a href="/orchard">orchards</a> <a href="/varieties">varieties</a> <a href="/cultivation">cultivation</a>"#,
        ),
        page(
            "http://pomology.example/orchard",
            r#"<h1>Orchard</h1><p>An orchard is a planting of fruit trees; soil and rootstock matter.</p>
               <a href="/soil">soil</a> <a href="/rootstock">rootstock</a>"#,
        ),
        page(
            "http://pomology.example/varieties",
            r#"<h1>Varieties</h1><p>Apple cultivars include honeycrisp and fuji.</p>
               <a href="/honeycrisp">honeycrisp</a> <a href="/fuji">fuji</a>"#,
        ),
        page(
            "http://pomology.example/cultivation",
            r#"<h1>Cultivation</h1><p>Grafting, pruning, and pollination.</p> <a href="/orchard">orchard</a>"#,
        ),
        page(
            "http://pomology.example/soil",
            r#"<h1>Soil</h1><p>Loam and drainage for healthy roots.</p>"#,
        ),
        page(
            "http://pomology.example/rootstock",
            r#"<h1>Rootstock</h1><p>Dwarfing rootstocks control tree size.</p>"#,
        ),
        page(
            "http://pomology.example/honeycrisp",
            r#"<h1>Honeycrisp</h1><p>A crisp, sweet apple cultivar.</p>"#,
        ),
        page(
            "http://pomology.example/fuji",
            r#"<h1>Fuji</h1><p>A dense, sweet apple cultivar.</p>"#,
        ),
        page(
            "http://tropical.example/banana",
            r#"<h1>Banana</h1><p>Bananas are tropical herbs.</p> <a href="/plantain">plantain</a>"#,
        ),
        page(
            "http://tropical.example/plantain",
            r#"<h1>Plantain</h1><p>A starchy cooking banana.</p>"#,
        ),
    ];

    let seeds = pages.iter().map(|p| p.url.clone()).collect();
    let output = build_v2_fixture_crawl(CrawlRequest::new("serp-preview", seeds), &pages)
        .expect("fixture crawl should build");
    let mut store = InMemoryGraphStore::new();
    output
        .graph
        .apply_to_store(&mut store)
        .expect("apply to store should succeed");

    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/rustyweb-serp.html".to_string());
    let query = env::args().nth(2).unwrap_or_else(|| "apple".to_string());

    let search = search_substrate(&store, &query, SearchOptions::default());
    let html = render_serp_html(&search);
    fs::write(&path, &html).expect("write serp html");

    eprintln!(
        "SERP for '{}': {} matches, {} pages in neighbourhood, {} links -> {} ({} bytes)",
        query,
        search.matched_count,
        search.kept_count,
        search.links.len(),
        path,
        html.len()
    );
}
