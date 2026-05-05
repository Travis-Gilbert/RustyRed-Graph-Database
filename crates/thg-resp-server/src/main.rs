mod protocol;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = std::env::var("THG_RESP_ADDR").unwrap_or_else(|_| "127.0.0.1:6380".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("THG_RESP_READY {}", addr);
    loop {
        let (_socket, _peer) = listener.accept().await?;
    }
}
