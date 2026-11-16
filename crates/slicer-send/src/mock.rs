//! A stand-in for a printer's HTTP API, for tests only (feature `mock`).
//!
//! It listens on a free port of 127.0.0.1, answers exactly one request with
//! a fixed status and body, and hands the request back for inspection.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;
use std::time::Duration;

/// One HTTP request as received.
#[derive(Debug, Clone)]
pub struct Request {
    /// Method, e.g. `POST`.
    pub method: String,
    /// Path with query, e.g. `/server/files/upload`.
    pub path: String,
    /// Header names in lower case, with their values.
    pub headers: Vec<(String, String)>,
    /// The body (read by `Content-Length`).
    pub body: Vec<u8>,
}

/// One part of a `multipart/form-data` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The `name` of the part.
    pub name: String,
    /// The `filename` of the part, if it has one.
    pub file_name: Option<String>,
    /// The content of the part.
    pub content: Vec<u8>,
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The quoted value of `key="..."` in a part header.
fn attribute(head: &str, key: &str) -> Option<String> {
    let start = head.find(&format!("; {key}=\""))? + key.len() + 4;
    let length = head[start..].find('"')?;
    Some(head[start..start + length].to_string())
}

impl Request {
    /// The value of a header, by name in any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The parts of a `multipart/form-data` body, in order; empty for any
    /// other body.
    pub fn fields(&self) -> Vec<Field> {
        let Some((_, boundary)) = self
            .header("content-type")
            .and_then(|value| value.split_once("boundary="))
        else {
            return Vec::new();
        };
        let delimiter = format!("--{}", boundary.trim_matches('"'));
        let mut fields = Vec::new();
        let mut rest = self.body.as_slice();
        while let Some(at) = find(rest, delimiter.as_bytes()) {
            rest = &rest[at + delimiter.len()..];
            let Some(part) = rest.strip_prefix(b"\r\n") else {
                break; // "--" after the last delimiter
            };
            let Some(end) = find(part, delimiter.as_bytes()) else {
                break;
            };
            let part = &part[..end];
            let part = part.strip_suffix(b"\r\n").unwrap_or(part);
            let Some(split) = find(part, b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&part[..split]);
            fields.push(Field {
                name: attribute(&head, "name").unwrap_or_default(),
                file_name: attribute(&head, "filename"),
                content: part[split + 4..].to_vec(),
            });
        }
        fields
    }

    /// The content of the first part called `name`.
    pub fn field(&self, name: &str) -> Option<Vec<u8>> {
        self.fields()
            .into_iter()
            .find(|field| field.name == name)
            .map(|field| field.content)
    }
}

/// A printer API that answers one request.
pub struct Printer {
    /// `host:port` it listens on.
    pub address: String,
    handle: JoinHandle<Option<Request>>,
}

impl Printer {
    /// Starts listening; the one request gets `status` with `body` (JSON).
    pub fn start(status: u16, body: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?.to_string();
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().ok()?;
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .ok()?;
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).ok()?;
            let mut words = line.split_whitespace();
            let method = words.next()?.to_string();
            let path = words.next()?.to_string();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).ok()?;
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                let (key, value) = line.split_once(':')?;
                headers.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
            let length = headers
                .iter()
                .find(|(key, _)| key == "content-length")
                .and_then(|(_, value)| value.parse().ok())
                .unwrap_or(0);
            let mut request = Request {
                method,
                path,
                headers,
                body: vec![0; length],
            };
            reader.read_exact(&mut request.body).ok()?;
            let answer = format!(
                "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(answer.as_bytes()).ok()?;
            Some(request)
        });
        Ok(Self { address, handle })
    }

    /// Waits for the request; `None` if none arrived or it was unreadable.
    pub fn request(self) -> Option<Request> {
        self.handle.join().ok().flatten()
    }
}
