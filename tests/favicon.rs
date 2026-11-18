//! The web page's icon is served (the page links it) and is a PNG.
use tenrec::server::{start, Config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn the_page_links_its_icon_and_the_server_serves_it() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(Config { port: 0, data_dir: dir.path().join("data"), bin: None, mcp_bin: None, extract: Default::default(), propose: Default::default(), sched: Default::default() }).await.unwrap();
    let get = |path: &'static str| {
        let addr = server.addr;
        async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", addr.port()).as_bytes()).await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            bytes
        }
    };
    let icon = get("/favicon.png").await;
    let text = String::from_utf8_lossy(&icon[..icon.len().min(300)]).to_ascii_lowercase();
    assert!(text.starts_with("http/1.1 200") && text.contains("content-type: image/png"), "{text}");
    assert!(icon.windows(8).any(|w| w == b"\x89PNG\r\n\x1a\n"), "the body is a PNG");
    let page = String::from_utf8_lossy(&get("/").await).into_owned();
    assert!(page.contains("rel=\"icon\"") && page.contains("/favicon.png"), "the page links its icon");
}
