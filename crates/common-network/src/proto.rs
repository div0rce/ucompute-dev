//! Control protocol messages (ADR-0003 D5): length-prefixed protobuf, 1 MiB bound. STUB.
//! Mirrors `proto/common/v1/control.proto`; hand-maintained prost structs (no protoc at build time).
//! Unknown fields are tolerated by prost decoding (F-08-10).
use common_core::Result;

#[derive(Clone, PartialEq, prost::Message)]
pub struct Hello {
    #[prost(uint32, tag = "1")]
    pub major: u32,
    #[prost(uint32, tag = "2")]
    pub minor: u32,
    #[prost(string, tag = "3")]
    pub common_version: String,
    #[prost(bytes = "vec", tag = "4")]
    pub peer_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "5")]
    pub nonce: Vec<u8>,
}

/// Signed capability record (F-02-04). `hardware_json` is the exact `common hardware --json` document.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Capabilities {
    #[prost(uint32, tag = "1")]
    pub schema_version: u32,
    #[prost(bytes = "vec", tag = "2")]
    pub peer_id: Vec<u8>,
    #[prost(string, tag = "3")]
    pub common_version: String,
    #[prost(string, tag = "4")]
    pub hardware_json: String,
    #[prost(uint64, tag = "5")]
    pub free_memory_bytes: u64,
    #[prost(string, repeated, tag = "6")]
    pub engines: Vec<String>,
    #[prost(string, repeated, tag = "7")]
    pub cached_model_sha256: Vec<String>,
    #[prost(string, repeated, tag = "8")]
    pub groups: Vec<String>,
    #[prost(uint64, tag = "9")]
    pub issued_at: u64,
    #[prost(bytes = "vec", tag = "10")]
    pub signature: Vec<u8>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct Status {
    #[prost(uint64, tag = "1")]
    pub free_memory_bytes: u64,
    #[prost(uint32, tag = "2")]
    pub active_sessions: u32,
    #[prost(uint32, tag = "3")]
    pub queue_depth: u32,
    #[prost(string, optional, tag = "4")]
    pub loaded_model_id: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ErrorMsg {
    #[prost(string, tag = "1")]
    pub code: String,
    #[prost(string, tag = "2")]
    pub message: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct RevocationMsg {
    #[prost(uint64, tag = "1")]
    pub seq: u64,
    #[prost(bytes = "vec", tag = "2")]
    pub peer_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub revoked_by: Vec<u8>,
    #[prost(string, tag = "4")]
    pub at_utc: String,
    #[prost(bytes = "vec", tag = "5")]
    pub signature: Vec<u8>,
}

/// Mode B request: run a whole generation on the peer. `request_json` = common_runtime::GenerateRequest.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RemoteRequest {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(string, tag = "2")]
    pub model_id: String,
    #[prost(string, tag = "3")]
    pub model_sha256: String,
    #[prost(string, tag = "4")]
    pub request_json: String,
    #[prost(string, tag = "5")]
    pub privacy: String,
}

/// One streamed event back: `event_json` = common_runtime::GenerateEvent or an ErrorMsg.
#[derive(Clone, PartialEq, prost::Message)]
pub struct RemoteChunk {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(string, tag = "2")]
    pub event_json: String,
    #[prost(bool, tag = "3")]
    pub done: bool,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ControlMessage {
    #[prost(oneof = "control_message::Body", tags = "1, 2, 3, 4, 5, 6, 7, 8")]
    pub body: Option<control_message::Body>,
}
pub mod control_message {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Body {
        #[prost(message, tag = "1")]
        Hello(super::Hello),
        #[prost(message, tag = "2")]
        Capabilities(super::Capabilities),
        #[prost(message, tag = "3")]
        Status(super::Status),
        #[prost(message, tag = "4")]
        Error(super::ErrorMsg),
        #[prost(message, tag = "5")]
        Revocation(super::RevocationMsg),
        #[prost(message, tag = "6")]
        RemoteRequest(super::RemoteRequest),
        #[prost(message, tag = "7")]
        RemoteChunk(super::RemoteChunk),
        #[prost(message, tag = "8")]
        Cancel(super::Cancel),
    }
}
#[derive(Clone, PartialEq, prost::Message)]
pub struct Cancel {
    #[prost(string, tag = "1")]
    pub request_id: String,
}

/// `u32 BE length || protobuf`. Rejects frames over `bound` with FrameTooLarge before allocating.
pub fn encode_frame(msg: &ControlMessage) -> Vec<u8> {
    let _ = msg;
    Vec::new()
}
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(r: &mut R, bound: usize) -> Result<Option<ControlMessage>> {
    let _ = (r, bound);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "proto",
    ))
}
pub async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, msg: &ControlMessage) -> Result<()> {
    let _ = (w, msg);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "proto",
    ))
}
