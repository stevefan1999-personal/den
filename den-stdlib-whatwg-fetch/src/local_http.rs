//! Loopback HTTP/1.1 listener for crate tests. Never leaves 127.0.0.1.

use std::{collections::HashMap, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[allow(dead_code)]
pub struct Incoming {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Outgoing {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Write the response, then leave the socket open.
    pub hang: bool,
    /// Accept the request and never write a response (abort tests).
    pub silent: bool,
}

impl Outgoing {
    pub fn ok(body: impl Into<Vec<u8>>, content_type: &str) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
            hang: false,
            silent: false,
        }
    }
}

pub struct LocalServer {
    pub port: u16,
}

impl LocalServer {
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

pub async fn serve(handler: impl Fn(Incoming) -> Outgoing + Send + Sync + 'static) -> LocalServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback bind");
    let port = listener.local_addr().expect("local addr").port();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                if let Err(error) = handle(stream, handler).await {
                    eprintln!("local_http: {error}");
                }
            });
        }
    });
    LocalServer { port }
}

async fn handle(
    mut stream: TcpStream,
    handler: Arc<dyn Fn(Incoming) -> Outgoing + Send + Sync>,
) -> std::io::Result<()> {
    let incoming = read_request(&mut stream).await?;
    let outgoing = handler(incoming);
    if outgoing.silent {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        return Ok(());
    }
    write_response(&mut stream, &outgoing).await?;
    if !outgoing.hang {
        let _ = stream.shutdown().await;
    }
    Ok(())
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Incoming> {
    let mut buf = Vec::new();
    loop {
        let mut tmp = [0u8; 1024];
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 1024 * 1024 {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "no header terminator")
        })?;
    let header_text = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0usize);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut tmp = [0u8; 1024];
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Ok(Incoming {
        method,
        path,
        headers,
        body,
    })
}

async fn write_response(stream: &mut TcpStream, outgoing: &Outgoing) -> std::io::Result<()> {
    let reason = match outgoing.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", outgoing.status);
    let mut has_length = false;
    for (name, value) in &outgoing.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_length = true;
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    if !has_length {
        head.push_str(&format!("Content-Length: {}\r\n", outgoing.body.len()));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&outgoing.body).await?;
    stream.flush().await
}
