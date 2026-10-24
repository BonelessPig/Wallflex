//! Discord: the local RPC protocol, the session that follows your call
//! and notifications, and the data the Discord widget draws. All safe
//! code; the pipe, HTTPS and window focusing are in `win32::discord`.

pub mod bot;
pub mod model;
pub mod rpc;
pub mod session;
