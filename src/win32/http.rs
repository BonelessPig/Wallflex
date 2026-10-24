//! HTTPS requests through `WinHTTP`, which is built into Windows: no TLS
//! library to ship. Blocking; call from a background thread.

use std::ffi::c_void;
use std::fmt::Write as _;

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_FLAG_SECURE, WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    WINHTTP_WEB_SOCKET_BUFFER_TYPE, WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE,
    WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS,
    WINHTTP_WEB_SOCKET_UTF8_FRAGMENT_BUFFER_TYPE,
    WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE, WinHttpCloseHandle,
    WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts, WinHttpWebSocketClose,
    WinHttpWebSocketCompleteUpgrade, WinHttpWebSocketQueryCloseStatus,
    WinHttpWebSocketReceive, WinHttpWebSocketSend,
};
use windows::core::{HSTRING, PCWSTR, w};

/// Timeouts for resolving, connecting, sending and receiving, in ms.
const TIMEOUT_MS: i32 = 15_000;
/// Read buffer size.
const CHUNK: usize = 16 * 1024;
/// Largest response accepted, so a bad server can't exhaust memory.
const MAX_BODY: usize = 8 * 1024 * 1024;

/// A finished response.
pub struct Response {
    /// HTTP status code.
    pub status: u32,
    /// The body.
    pub body: Vec<u8>,
}

/// A `WinHTTP` handle, closed on drop.
struct Handle(*mut c_void);

impl Handle {
    /// Wraps a handle, or reports the last error if it's null.
    fn new(raw: *mut c_void, what: &str) -> Result<Self, String> {
        if raw.is_null() {
            let error = windows::core::Error::from_thread();
            return Err(format!("{what} failed: {error}"));
        }
        Ok(Self(raw))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from `WinHTTP` and is closed once.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// Sends an HTTPS request to `host` and returns the response.
/// `headers` are `Name: value` lines separated by `\r\n`.
///
/// # Errors
///
/// A message when the connection or request fails.
pub fn request(
    method: &str,
    host: &str,
    path: &str,
    headers: &str,
    body: &[u8],
) -> Result<Response, String> {
    let body_len = u32::try_from(body.len()).map_err(|e| e.to_string())?;
    // SAFETY: plain values and constant strings.
    let session = Handle::new(
        unsafe {
            WinHttpOpen(
                w!("Wallflex/0.1"),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        },
        "WinHttpOpen",
    )?;
    // SAFETY: the session handle is valid.
    unsafe {
        WinHttpSetTimeouts(
            session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS,
        )
    }
    .map_err(|e| e.to_string())?;
    let host = HSTRING::from(host);
    // SAFETY: the session handle is valid and `host` outlives the call.
    let connection = Handle::new(
        unsafe {
            WinHttpConnect(session.0, &host, INTERNET_DEFAULT_HTTPS_PORT, 0)
        },
        "WinHttpConnect",
    )?;
    let method = HSTRING::from(method);
    let path = HSTRING::from(path);
    // SAFETY: the connection is valid; the strings outlive the call; no
    // accept types (null) means any.
    let request = Handle::new(
        unsafe {
            WinHttpOpenRequest(
                connection.0,
                &method,
                &path,
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        },
        "WinHttpOpenRequest",
    )?;
    let headers: Vec<u16> = headers.encode_utf16().collect();
    let headers = (!headers.is_empty()).then_some(headers.as_slice());
    let body_ptr = (!body.is_empty()).then(|| body.as_ptr().cast::<c_void>());
    // SAFETY: the request is valid and the headers and body outlive the
    // call (it's synchronous).
    unsafe {
        WinHttpSendRequest(request.0, headers, body_ptr, body_len, body_len, 0)
    }
    .map_err(|e| format!("sending to Discord failed: {e}"))?;
    // SAFETY: the request was sent; the reserved argument must be null.
    unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }
        .map_err(|e| format!("no response: {e}"))?;
    let status = status_code(&request)?;
    let body = read_body(&request)?;
    Ok(Response { status, body })
}

/// The response's status code.
fn status_code(request: &Handle) -> Result<u32, String> {
    let mut status = 0_u32;
    let mut size = super::struct_size::<u32>();
    let mut index = 0_u32;
    // SAFETY: `status` is a u32 and `size` says so, as the
    // FLAG_NUMBER query expects.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(std::ptr::from_mut(&mut status).cast()),
            &raw mut size,
            &raw mut index,
        )
    }
    .map_err(|e| e.to_string())?;
    Ok(status)
}

/// Reads the whole body.
fn read_body(request: &Handle) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    let mut chunk = vec![0_u8; CHUNK];
    let chunk_len = u32::try_from(chunk.len()).map_err(|e| e.to_string())?;
    loop {
        let mut read = 0_u32;
        // SAFETY: `chunk` has room for `chunk_len` bytes and `read`
        // receives how many were written.
        unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk_len,
                &raw mut read,
            )
        }
        .map_err(|e| format!("reading the response failed: {e}"))?;
        let read = usize::try_from(read).map_err(|e| e.to_string())?;
        if read == 0 {
            return Ok(body);
        }
        body.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if body.len() > MAX_BODY {
            return Err("response too large".into());
        }
    }
}

/// `application/x-www-form-urlencoded` encoding of `fields`.
pub fn form_encode(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encodes everything but unreserved characters.
fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Wait forever (for a websocket that's quiet between heartbeats).
const NO_TIMEOUT: i32 = 0;
/// Largest websocket message accepted (a big server's member list can
/// be several megabytes).
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// The normal close code.
const NORMAL_CLOSE: u16 = 1000;

/// What a websocket read produced.
pub enum Received {
    /// A whole text message.
    Text(String),
    /// The server closed the connection with this code.
    Closed(u16),
}

/// A secure websocket. One thread may send while another receives, as
/// `WinHTTP` allows.
pub struct WebSocket {
    /// The websocket handle.
    socket: Handle,
    /// Kept open for the socket's lifetime.
    _connection: Handle,
    /// Kept open for the socket's lifetime.
    _session: Handle,
}

// SAFETY: WinHTTP websocket handles may be used from any thread, with
// one send and one receive outstanding at a time; Wallflex sends from one
// thread and receives on another.
unsafe impl Send for WebSocket {}
// SAFETY: as above.
unsafe impl Sync for WebSocket {}

impl WebSocket {
    /// Connects to `wss://host/path`.
    ///
    /// # Errors
    ///
    /// A message when the connection or upgrade fails.
    pub fn connect(host: &str, path: &str) -> Result<Self, String> {
        // SAFETY: plain values and constant strings.
        let session = Handle::new(
            unsafe {
                WinHttpOpen(
                    w!("Wallflex/0.1"),
                    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    0,
                )
            },
            "WinHttpOpen",
        )?;
        // SAFETY: the session handle is valid.
        unsafe {
            WinHttpSetTimeouts(
                session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, NO_TIMEOUT,
            )
        }
        .map_err(|e| e.to_string())?;
        let host = HSTRING::from(host);
        // SAFETY: the session is valid and `host` outlives the call.
        let connection = Handle::new(
            unsafe {
                WinHttpConnect(session.0, &host, INTERNET_DEFAULT_HTTPS_PORT, 0)
            },
            "WinHttpConnect",
        )?;
        let path = HSTRING::from(path);
        // SAFETY: the connection is valid and the strings outlive the call.
        let request = Handle::new(
            unsafe {
                WinHttpOpenRequest(
                    connection.0,
                    w!("GET"),
                    &path,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    std::ptr::null(),
                    WINHTTP_FLAG_SECURE,
                )
            },
            "WinHttpOpenRequest",
        )?;
        // SAFETY: the request is valid; this option takes no buffer.
        unsafe {
            WinHttpSetOption(
                Some(request.0.cast_const()),
                WINHTTP_OPTION_UPGRADE_TO_WEB_SOCKET,
                None,
            )
        }
        .map_err(|e| format!("websocket upgrade refused: {e}"))?;
        // SAFETY: the request is valid; no headers or body.
        unsafe { WinHttpSendRequest(request.0, None, None, 0, 0, 0) }
            .map_err(|e| format!("connecting failed: {e}"))?;
        // SAFETY: the request was sent; the reserved argument must be null.
        unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }
            .map_err(|e| format!("no response: {e}"))?;
        // SAFETY: the request got its response; it's closed after the
        // upgrade, as WinHTTP expects.
        let socket = Handle::new(
            unsafe {
                WinHttpWebSocketCompleteUpgrade(request.0.cast_const(), None)
            },
            "WinHttpWebSocketCompleteUpgrade",
        )?;
        drop(request);
        Ok(Self {
            socket,
            _connection: connection,
            _session: session,
        })
    }

    /// Sends a text message.
    ///
    /// # Errors
    ///
    /// A message when the socket is closed.
    pub fn send_text(&self, text: &str) -> Result<(), String> {
        // SAFETY: the socket is valid and the bytes outlive the call.
        let code = unsafe {
            WinHttpWebSocketSend(
                self.socket.0.cast_const(),
                WINHTTP_WEB_SOCKET_UTF8_MESSAGE_BUFFER_TYPE,
                Some(text.as_bytes()),
            )
        };
        win32_result(code, "sending")
    }

    /// Waits for the next whole text message (or the close).
    ///
    /// # Errors
    ///
    /// A message when the connection drops.
    pub fn receive(&self) -> Result<Received, String> {
        let mut message = Vec::new();
        let mut chunk = vec![0_u8; CHUNK];
        let chunk_len =
            u32::try_from(chunk.len()).map_err(|e| e.to_string())?;
        loop {
            let mut read = 0_u32;
            let mut kind = WINHTTP_WEB_SOCKET_BUFFER_TYPE::default();
            // SAFETY: `chunk` has room for `chunk_len` bytes; `read` and
            // `kind` receive what arrived.
            let code = unsafe {
                WinHttpWebSocketReceive(
                    self.socket.0.cast_const(),
                    chunk.as_mut_ptr().cast(),
                    chunk_len,
                    &raw mut read,
                    &raw mut kind,
                )
            };
            win32_result(code, "receiving")?;
            if kind == WINHTTP_WEB_SOCKET_CLOSE_BUFFER_TYPE {
                return Ok(Received::Closed(self.close_status()));
            }
            let read = usize::try_from(read).map_err(|e| e.to_string())?;
            message.extend_from_slice(chunk.get(..read).unwrap_or_default());
            if message.len() > MAX_MESSAGE {
                return Err("message too large".into());
            }
            if kind != WINHTTP_WEB_SOCKET_UTF8_FRAGMENT_BUFFER_TYPE {
                return String::from_utf8(message)
                    .map(Received::Text)
                    .map_err(|e| e.to_string());
            }
        }
    }

    /// Closes the socket; a blocked `receive` then returns.
    pub fn close(&self) {
        let status = u16::try_from(WINHTTP_WEB_SOCKET_SUCCESS_CLOSE_STATUS.0)
            .unwrap_or(NORMAL_CLOSE);
        // SAFETY: the socket is valid; no reason text. Failure means it
        // was already closed.
        let _ = unsafe {
            WinHttpWebSocketClose(self.socket.0.cast_const(), status, None, 0)
        };
    }

    /// The close code the server sent.
    fn close_status(&self) -> u16 {
        let mut status = 0_u16;
        let mut consumed = 0_u32;
        // SAFETY: `status` and `consumed` are valid places to write to; no
        // reason buffer.
        let _ = unsafe {
            WinHttpWebSocketQueryCloseStatus(
                self.socket.0.cast_const(),
                &raw mut status,
                None,
                0,
                &raw mut consumed,
            )
        };
        status
    }
}

/// A Win32 error code as a result.
fn win32_result(code: u32, what: &str) -> Result<(), String> {
    if code == 0 {
        return Ok(());
    }
    let error = windows::core::Error::from(
        windows::Win32::Foundation::WIN32_ERROR(code),
    );
    Err(format!("{what} failed: {}", error.message()))
}

#[cfg(test)]
mod tests {
    use super::form_encode;

    #[test]
    fn encodes_forms() {
        assert_eq!(
            form_encode(&[("a", "b c"), ("redirect_uri", "http://localhost")]),
            "a=b%20c&redirect_uri=http%3A%2F%2Flocalhost"
        );
    }
}
