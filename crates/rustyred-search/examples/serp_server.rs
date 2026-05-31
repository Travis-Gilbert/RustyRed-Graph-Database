//! Live SERP server — run the browser's search-as-graph surface as a real HTTP
//! endpoint so it can be opened in a browser today, before the Servo embedder's
//! window lands.
//!
//! This is a dev / reference harness (std-only, no framework), not the production
//! endpoint: it seeds a fixture substrate via the real `build_v2_fixture_crawl`
//! write path, then serves the REAL renderer over HTTP:
//!
//!   GET /search?q=<query>   -> render_serp_html(search_substrate(store, q))  (text/html)
//!   GET /search.json?q=...  -> the SubstrateSearch payload                   (application/json)
//!   GET /                   -> browse-mode SERP (empty query)
//!
//! The request handler here is exactly the shape `apps/browser` (Codex's embedder
//! lane) mirrors: intercept a search URL -> search_substrate -> render_serp_html
//! -> serve. Against a real crawled store the same handler returns real results.
//!
//!   cargo run -p rustyred-search --example serp_server          # http://127.0.0.1:8088
//!   open http://127.0.0.1:8088/search?q=apple

use std::env;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use rustyred_core::graph_store::InMemoryGraphStore;
use rustyred_search::{
    build_v2_fixture_crawl, render_serp_html, search_substrate, serp_payload_json, CrawlRequest,
    FetchedPage, SearchOptions,
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

/// The same fixture "pomology" web the preview harness uses: an interlinked apple
/// cluster plus an unrelated tropical cluster.
fn seed_store() -> InMemoryGraphStore {
    let pages = vec![
        page(
            "http://pomology.example/apple",
            r#"<h1>Apple</h1><p>The apple is a pome fruit; apple cultivation spans varieties and orchards.</p>
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
            r#"<h1>Soil</h1><p>Loam and drainage.</p>"#,
        ),
        page(
            "http://pomology.example/rootstock",
            r#"<h1>Rootstock</h1><p>Dwarfing rootstocks.</p>"#,
        ),
        page(
            "http://pomology.example/honeycrisp",
            r#"<h1>Honeycrisp</h1><p>A crisp apple cultivar.</p>"#,
        ),
        page(
            "http://pomology.example/fuji",
            r#"<h1>Fuji</h1><p>A sweet apple cultivar.</p>"#,
        ),
        page(
            "http://tropical.example/banana",
            r#"<h1>Banana</h1><p>Bananas are tropical herbs.</p> <a href="/plantain">plantain</a>"#,
        ),
        page(
            "http://tropical.example/plantain",
            r#"<h1>Plantain</h1><p>A cooking banana.</p>"#,
        ),
    ];
    let seeds = pages.iter().map(|p| p.url.clone()).collect();
    let output =
        build_v2_fixture_crawl(CrawlRequest::new("serp-server", seeds), &pages).expect("crawl");
    let mut store = InMemoryGraphStore::new();
    output.graph.apply_to_store(&mut store).expect("apply");
    store
}

/// Minimal percent-decoding for the `q` query parameter (`+` -> space, `%XX`).
fn url_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extract path + the `q` parameter from a request target like `/search?q=apple`.
fn parse_target(target: &str) -> (String, String) {
    let (path, query_string) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q),
        None => (target.to_string(), ""),
    };
    let mut q = String::new();
    for pair in query_string.split('&') {
        if let Some(value) = pair.strip_prefix("q=") {
            q = url_decode(value);
        }
    }
    (path, q)
}

fn write_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn handle(stream: &mut TcpStream, store: &InMemoryGraphStore) {
    let mut buf = [0u8; 4096];
    let read = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return,
    };
    let request = String::from_utf8_lossy(&buf[..read]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let (path, query) = parse_target(target);

    // The handler Codex's Servo interception mirrors: query -> search -> render.
    let search = search_substrate(store, &query, SearchOptions::default());
    match path.as_str() {
        "/search.json" => write_response(
            stream,
            "200 OK",
            "application/json",
            &serp_payload_json(&search),
        ),
        "/favicon.ico" => write_response(stream, "404 Not Found", "text/plain", "no favicon"),
        _ => write_response(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            &render_serp_html(&search),
        ),
    }

    eprintln!(
        "{} q={:?} -> {} matches / {} nodes / {} links",
        path,
        query,
        search.matched_count,
        search.kept_count,
        search.links.len()
    );
}

fn main() {
    let port: u16 = env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8088);
    let store = seed_store();
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!("SERP server live: http://{addr}/search?q=apple  (Ctrl-C to stop)");
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => handle(&mut stream, &store),
            Err(e) => eprintln!("connection error: {e}"),
        }
    }
}
