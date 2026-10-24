//! Just enough HTTP/1.1 for one local page: read a request, write a
//! response, close the connection. No keep-alive, no chunked bodies.

use std::fmt;
use std::io::{Read, Write};

/// Largest header block accepted, in bytes.
const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Largest body accepted, in bytes (a config file is a few KB).
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Bytes read from the socket at a time.
const CHUNK_BYTES: usize = 4096;
/// Where the headers end.
const HEADER_END: &[u8] = b"\r\n\r\n";

/// One request.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Request {
    /// `GET`, `POST`, ...
    pub method: String,
    /// The path, without the query.
    pub path: String,
    /// The query, without the `?` (empty if none).
    pub query: String,
    /// Headers, names in lower case.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl Request {
    /// A header's value, by lower-case name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// A query parameter's value (not percent-decoded; the values used
    /// here are plain letters and digits).
    pub fn param(&self, name: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v)
    }
}

/// Why a request couldn't be read.
#[derive(Debug)]
pub enum HttpError {
    /// The connection failed or closed early.
    Io(std::io::Error),
    /// Not a request this server understands.
    Malformed(&'static str),
    /// Headers or body too big.
    TooLarge,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Malformed(what) => write!(f, "bad request: {what}"),
            Self::TooLarge => f.write_str("request too large"),
        }
    }
}

impl From<std::io::Error> for HttpError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Reads one request from `stream`.
///
/// # Errors
///
/// An [`HttpError`] if the connection fails or the request is bad or
/// too large.
pub fn read_request<R: Read>(stream: &mut R) -> Result<Request, HttpError> {
    let mut data = Vec::new();
    let mut chunk = [0_u8; CHUNK_BYTES];
    let header_len = loop {
        if let Some(at) = find(&data, HEADER_END) {
            break at;
        }
        if data.len() > MAX_HEADER_BYTES {
            return Err(HttpError::TooLarge);
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(HttpError::Malformed("closed before the headers"));
        }
        data.extend_from_slice(chunk.get(..read).unwrap_or_default());
    };
    let head = data.get(..header_len).unwrap_or_default();
    let head = std::str::from_utf8(head)
        .map_err(|_| HttpError::Malformed("headers aren't text"))?;
    let mut request = parse_head(head)?;
    let length = match request.header("content-length") {
        Some(value) => value
            .trim()
            .parse::<usize>()
            .map_err(|_| HttpError::Malformed("content-length"))?,
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        return Err(HttpError::TooLarge);
    }
    let body_start = header_len.saturating_add(HEADER_END.len());
    let mut body = data.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(HttpError::Malformed("closed before the body"));
        }
        body.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
    body.truncate(length);
    request.body = body;
    Ok(request)
}

/// Parses the request line and headers.
fn parse_head(head: &str) -> Result<Request, HttpError> {
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or_default();
    let mut words = line.split(' ');
    let (Some(method), Some(target), Some(version)) =
        (words.next(), words.next(), words.next())
    else {
        return Err(HttpError::Malformed("request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Malformed("not HTTP/1"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Ok(Request {
        method: method.to_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        body: Vec::new(),
    })
}

/// Where `needle` first starts in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A response to send.
pub struct Response {
    /// Status code.
    pub status: u16,
    /// `Content-Type`.
    pub content_type: &'static str,
    /// The body.
    pub body: Vec<u8>,
}

impl Response {
    /// A JSON response.
    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        }
    }

    /// A plain-text response, for errors.
    pub fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: text.as_bytes().to_vec(),
        }
    }

    /// Writes it, with headers that keep the page private: nothing is
    /// cached, sniffed, framed or sent on as a referrer.
    ///
    /// # Errors
    ///
    /// The socket's error, if writing fails.
    pub fn write_to<W: Write>(&self, stream: &mut W) -> std::io::Result<()> {
        let head = format!(
            "HTTP/1.1 {} {}\r\n\
             Content-Type: {}\r\n\
             Content-Length: {}\r\n\
             Cache-Control: no-store\r\n\
             X-Content-Type-Options: nosniff\r\n\
             X-Frame-Options: DENY\r\n\
             Referrer-Policy: no-referrer\r\n\
             Content-Security-Policy: default-src 'self'; \
             script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
             img-src 'self' blob: data:; frame-ancestors 'none'\r\n\
             Connection: close\r\n\r\n",
            self.status,
            reason(self.status),
            self.content_type,
            self.body.len(),
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(&self.body)?;
        stream.flush()
    }
}

/// The reason phrase for the status codes used here.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::{Response, read_request};

    #[test]
    fn reads_a_post_with_a_body() {
        let raw = b"POST /api/save?x=1&t=abc HTTP/1.1\r\n\
            Host: 127.0.0.1:5000\r\n\
            Content-Length: 5\r\n\r\nhello";
        let request = read_request(&mut &raw[..]).unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/save");
        assert_eq!(request.param("t"), Some("abc"));
        assert_eq!(request.param("y"), None);
        assert_eq!(request.header("host"), Some("127.0.0.1:5000"));
        assert_eq!(request.body, b"hello");
    }

    #[test]
    fn reads_a_get_without_a_body() {
        let raw = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        let request = read_request(&mut &raw[..]).unwrap();
        assert_eq!(request.path, "/");
        assert_eq!(request.body, b"");
    }

    #[test]
    fn rejects_bad_requests() {
        assert!(read_request(&mut &b"nonsense\r\n\r\n"[..]).is_err());
        assert!(read_request(&mut &b"GET / HTTP/1.1\r\n"[..]).is_err());
        let short = b"POST / HTTP/1.1\r\nContent-Length: 9\r\n\r\nhi";
        assert!(read_request(&mut &short[..]).is_err());
        let huge = b"POST / HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n";
        assert!(read_request(&mut &huge[..]).is_err());
    }

    #[test]
    fn writes_a_response() {
        let mut out = Vec::new();
        Response::text(404, "no").write_to(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.ends_with("\r\n\r\nno"));
    }
}
