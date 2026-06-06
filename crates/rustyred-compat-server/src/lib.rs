use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use rustyred_core::{RustyredExecutor, RustyredRequest};
use serde_json::{json, Value};

pub type SharedExecutor = Arc<Mutex<Box<dyn RustyredExecutor + Send>>>;

pub fn serve(listener: TcpListener, executor: SharedExecutor) -> std::io::Result<()> {
    for stream in listener.incoming() {
        let stream = stream?;
        let executor = Arc::clone(&executor);
        thread::spawn(move || {
            let _ = handle_stream(stream, executor);
        });
    }
    Ok(())
}

pub fn handle_stream(mut stream: TcpStream, executor: SharedExecutor) -> std::io::Result<()> {
    let request = read_request(&mut stream)?;
    let response = handle_http_request(&request, executor);
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

pub fn handle_http_request(raw: &str, executor: SharedExecutor) -> String {
    let parsed = match parse_request(raw) {
        Some(request) => request,
        None => return json_response(400, json!({ "ok": false, "error": "bad_request" })),
    };

    match (parsed.method.as_str(), parsed.path.as_str()) {
        ("GET", "/health") => json_response(200, json!({ "ok": true, "status": "healthy" })),
        ("GET", "/ready") => {
            let state_hash = executor.lock().unwrap().state().hash();
            json_response(
                200,
                json!({ "ok": true, "status": "ready", "state_hash": state_hash }),
            )
        }
        ("GET", "/v1/state/hash") => {
            let state_hash = executor.lock().unwrap().state().hash();
            json_response(200, json!({ "ok": true, "state_hash": state_hash }))
        }
        ("GET", path) if path.starts_with("/v1/runs/") => {
            let run_id = path.trim_start_matches("/v1/runs/");
            let request = RustyredRequest::new("RUSTYRED.RUN.GET", json!({ "run_id": run_id }));
            let response = executor.lock().unwrap().execute_request(request);
            json_response(200, serde_json::to_value(response).unwrap())
        }
        ("POST", "/v1/command") => command_response(&parsed.body, executor),
        ("POST", "/v1/batch") => batch_response(&parsed.body, executor),
        _ => json_response(404, json!({ "ok": false, "error": "not_found" })),
    }
}

fn command_response(body: &str, executor: SharedExecutor) -> String {
    let value = match serde_json::from_str::<Value>(body) {
        Ok(value) => value,
        Err(exc) => {
            return json_response(
                400,
                json!({ "ok": false, "error": "invalid_json", "message": exc.to_string() }),
            )
        }
    };
    let command = value
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("RUSTYRED.UNKNOWN");
    let args = value
        .get("args")
        .or_else(|| value.get("payload"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let request = RustyredRequest::new(command, args);
    let response = executor.lock().unwrap().execute_request(request);
    let status = if response.ok { 200 } else { 400 };
    json_response(status, serde_json::to_value(response).unwrap())
}

fn batch_response(body: &str, executor: SharedExecutor) -> String {
    let value = match serde_json::from_str::<Value>(body) {
        Ok(value) => value,
        Err(exc) => {
            return json_response(
                400,
                json!({ "ok": false, "error": "invalid_json", "message": exc.to_string() }),
            )
        }
    };
    let commands = value
        .get("commands")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut results = Vec::new();
    for item in commands {
        let command = item
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("RUSTYRED.UNKNOWN");
        let args = item
            .get("args")
            .or_else(|| item.get("payload"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let request = RustyredRequest::new(command, args);
        results.push(executor.lock().unwrap().execute_request(request));
    }
    let state_hash = executor.lock().unwrap().state().hash();
    json_response(
        200,
        json!({ "ok": true, "results": results, "state_hash": state_hash }),
    )
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buffer = [0_u8; 8192];
    let mut bytes = Vec::new();
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if request_complete(&bytes) {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn request_complete(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let Some((headers, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .or_else(|| {
            headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    body.len() >= content_length
}

struct ParsedRequest {
    method: String,
    path: String,
    body: String,
}

fn parse_request(raw: &str) -> Option<ParsedRequest> {
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    Some(ParsedRequest {
        method,
        path,
        body: body.to_string(),
    })
}

fn json_response(status: u16, body: Value) -> String {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let encoded = serde_json::to_string(&body).unwrap();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{encoded}",
        encoded.len()
    )
}

#[cfg(test)]
mod tests {
    use super::{handle_http_request, SharedExecutor};
    use rustyred_core::{InMemoryRustyredExecutor, RustyredExecutor, RustyredRequest};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[test]
    fn command_endpoint_executes_core_command() {
        let executor: SharedExecutor =
            Arc::new(Mutex::new(Box::new(InMemoryRustyredExecutor::new())));
        let body = r#"{"command":"RUSTYRED.RUN.BEGIN","args":{"run_id":"run:1","task":"server"}}"#;
        let raw = format!(
            "POST /v1/command HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let response = handle_http_request(&raw, executor);
        let json_text = response.split("\r\n\r\n").nth(1).unwrap();
        let parsed: Value = serde_json::from_str(json_text).unwrap();

        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["payload"]["run_id"], "run:1");
    }

    #[test]
    fn http_sequence_matches_direct_core_state_hash() {
        let executor: SharedExecutor =
            Arc::new(Mutex::new(Box::new(InMemoryRustyredExecutor::new())));
        let commands = [
            r#"{"command":"RUSTYRED.RUN.BEGIN","args":{"run_id":"run:1","task":"server"}}"#,
            r#"{"command":"RUSTYRED.RUN.STEP","args":{"run_id":"run:1","step_id":"step:1"}}"#,
            r#"{"command":"RUSTYRED.CONTEXT.PACK","args":{"artifact_id":"artifact:1","sections":[]}}"#,
            r#"{"command":"RUSTYRED.STATE.HASH","args":{}}"#,
        ];
        let mut http_hash = String::new();
        for body in commands {
            let raw = format!(
                "POST /v1/command HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let response = handle_http_request(&raw, Arc::clone(&executor));
            let json_text = response.split("\r\n\r\n").nth(1).unwrap();
            let parsed: Value = serde_json::from_str(json_text).unwrap();
            http_hash = parsed["state_hash"].as_str().unwrap().to_string();
        }

        let mut direct = InMemoryRustyredExecutor::new();
        for body in commands {
            let request: RustyredRequest = serde_json::from_str(body).unwrap();
            direct.execute_request(request);
        }

        assert_eq!(http_hash, direct.state_hash());
    }
}
