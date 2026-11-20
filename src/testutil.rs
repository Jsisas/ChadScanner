//! Test-only HTTP server: a scripted API on 127.0.0.1, one request per connection.
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A request as the server saw it.
#[derive(Debug, Clone, Default)]
pub struct Req {
    pub method: String,
    /// Path and query.
    pub path: String,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == &name.to_ascii_lowercase()).map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// A canned response.
pub struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Reply {
        Reply { status, headers: Vec::new(), body: body.into() }
    }

    pub fn json(status: u16, body: &Value) -> Reply {
        Reply::new(status, body.to_string()).header("Content-Type", "application/json")
    }

    pub fn header(mut self, name: &str, value: &str) -> Reply {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

pub struct MockServer {
    /// `http://127.0.0.1:<port>`
    pub url: String,
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
}

impl MockServer {
    /// Answers each request with `handler`; the server stops when dropped.
    pub fn start(handler: impl Fn(&Req) -> Reply + Send + 'static) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut conn) = conn else { continue };
                let Some(req) = read_request(&mut conn) else { continue };
                let reply = handler(&req);
                seen2.lock().unwrap().push(req);
                let _ = write_reply(&mut conn, &reply);
            }
        });
        MockServer { url: format!("http://{addr}"), addr, seen, stop }
    }

    /// Every request so far, oldest first.
    pub fn requests(&self) -> Vec<Req> {
        self.seen.lock().unwrap().clone()
    }

    /// Requests whose method and path (without query) match.
    pub fn count(&self, method: &str, path: &str) -> usize {
        self.requests().iter().filter(|r| r.method == method && r.path.split('?').next() == Some(path)).count()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the blocked accept.
        let _ = TcpStream::connect(self.addr);
    }
}

fn read_request(conn: &mut TcpStream) -> Option<Req> {
    let mut r = BufReader::new(conn.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let mut req = Req { method: parts.next()?.to_owned(), path: parts.next()?.to_owned(), ..Default::default() };
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        req.headers.push((k.to_ascii_lowercase(), v.trim().to_owned()));
    }
    let len = req.header("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    req.body = vec![0; len];
    r.read_exact(&mut req.body).ok()?;
    Some(req)
}

fn write_reply(conn: &mut TcpStream, reply: &Reply) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n", reply.status, reply.body.len());
    for (k, v) in &reply.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    conn.write_all(head.as_bytes())?;
    conn.write_all(&reply.body)?;
    conn.flush()
}
