//! Control protocol messages (ADR-0003 D5): length-prefixed protobuf, 1 MiB bound.
//! Mirrors `proto/common/v1/control.proto`; hand-maintained prost structs (no protoc at build time).
//! Unknown fields and unknown `oneof` variants are skipped by prost decoding (F-08-10), see
//! `unknown_field_tolerated` and `unknown_oneof_variant_tolerated`.
use crate::MAX_CONTROL_FRAME;
use common_core::{CommonError, ErrorCode, Result};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

/// Capability record (F-02-04). `hardware_json` is the exact `common hardware --json` document.
/// Travels only inside [`SignedCapabilities`]; tag 10 (the old in-record `signature`) is retired.
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
}

/// A `Capabilities` record as the exact bytes its sender signed, plus the Ed25519 signature over
/// them. The receiver verifies these bytes before decoding, so a newer minor that adds fields
/// still verifies on an older peer (prost drops unknown fields, a re-encoding would differ).
#[derive(Clone, PartialEq, prost::Message)]
pub struct SignedCapabilities {
    /// Protobuf encoding of `Capabilities`.
    #[prost(bytes = "vec", tag = "1")]
    pub record: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
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
    #[prost(oneof = "control_message::Body", tags = "1, 3, 4, 5, 6, 7, 8, 9")]
    pub body: Option<control_message::Body>,
}
pub mod control_message {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Body {
        #[prost(message, tag = "1")]
        Hello(super::Hello),
        /// Tag 2 (an unsigned-bytes `Capabilities`) is retired: an old peer sees an unknown
        /// variant and fails with "expected Capabilities", never a signature mismatch.
        #[prost(message, tag = "9")]
        Capabilities(super::SignedCapabilities),
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

impl From<control_message::Body> for ControlMessage {
    fn from(body: control_message::Body) -> Self {
        ControlMessage { body: Some(body) }
    }
}

/// `u32 BE length || protobuf`.
pub fn encode_frame(msg: &ControlMessage) -> Vec<u8> {
    let body = msg.encode_to_vec();
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

fn too_large(len: usize, bound: usize) -> CommonError {
    CommonError::new(
        ErrorCode::FrameTooLarge,
        format!("control frame of {len} bytes exceeds the {bound}-byte bound"),
    )
}

fn lost(e: std::io::Error) -> CommonError {
    CommonError::new(ErrorCode::PeerLost, format!("control stream: {e}"))
}

/// Reads one frame. `Ok(None)` on a clean end of stream before a length prefix. A length over
/// `bound` is rejected with `FrameTooLarge` before any body allocation.
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(r: &mut R, bound: usize) -> Result<Option<ControlMessage>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(lost(e)),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > bound {
        return Err(too_large(len, bound));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.map_err(lost)?;
    ControlMessage::decode(buf.as_slice())
        .map(Some)
        .map_err(|e| CommonError::invalid(format!("undecodable control frame: {e}")))
}

/// Writes one frame; refuses to send anything the peer would reject (`MAX_CONTROL_FRAME`).
pub async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, msg: &ControlMessage) -> Result<()> {
    let frame = encode_frame(msg);
    if frame.len() - 4 > MAX_CONTROL_FRAME {
        return Err(too_large(frame.len() - 4, MAX_CONTROL_FRAME));
    }
    w.write_all(&frame).await.map_err(lost)
}

#[cfg(test)]
mod tests {
    use super::control_message::Body;
    use super::*;

    async fn round_trip(body: Body) {
        let msg = ControlMessage::from(body);
        let bytes = encode_frame(&msg);
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        let mut r = bytes.as_slice();
        let back = read_frame(&mut r, MAX_CONTROL_FRAME).await.unwrap().unwrap();
        assert_eq!(back, msg);
        assert!(r.is_empty());
        // The same through write_frame.
        let mut w = Vec::new();
        write_frame(&mut w, &msg).await.unwrap();
        assert_eq!(w, bytes);
    }

    #[tokio::test]
    async fn every_message_round_trips() {
        round_trip(Body::Hello(Hello {
            major: 1,
            minor: 3,
            common_version: "0.1.0".into(),
            peer_id: vec![7; 32],
            nonce: vec![1, 2, 3],
        }))
        .await;
        let caps = Capabilities {
            schema_version: 1,
            peer_id: vec![9; 32],
            common_version: "0.1.0".into(),
            hardware_json: "{\"host\":{}}".into(),
            free_memory_bytes: 1 << 30,
            engines: vec!["llama.cpp b11609 macos-arm64".into()],
            cached_model_sha256: vec!["ab".into(), "cd".into()],
            groups: vec!["home".into()],
            issued_at: 1_760_000_000,
        };
        round_trip(Body::Capabilities(SignedCapabilities {
            record: caps.encode_to_vec(),
            signature: vec![0xAA; 64],
        }))
        .await;
        round_trip(Body::Status(Status {
            free_memory_bytes: 5,
            active_sessions: 1,
            queue_depth: 2,
            loaded_model_id: Some("stories15M-q4_0".into()),
        }))
        .await;
        round_trip(Body::Status(Status::default())).await;
        round_trip(Body::Error(ErrorMsg {
            code: "peer_lost".into(),
            message: "gone".into(),
        }))
        .await;
        round_trip(Body::Revocation(RevocationMsg {
            seq: 4,
            peer_id: vec![1; 32],
            revoked_by: vec![2; 32],
            at_utc: "2026-10-14T00:00:00Z".into(),
            signature: vec![3; 64],
        }))
        .await;
        round_trip(Body::RemoteRequest(RemoteRequest {
            request_id: "r1".into(),
            model_id: "m".into(),
            model_sha256: "66".into(),
            request_json: "{}".into(),
            privacy: "trusted_group".into(),
        }))
        .await;
        round_trip(Body::RemoteChunk(RemoteChunk {
            request_id: "r1".into(),
            event_json: "{\"token\":\"x\"}".into(),
            done: true,
        }))
        .await;
        round_trip(Body::Cancel(Cancel {
            request_id: "r1".into(),
        }))
        .await;
    }

    #[tokio::test]
    async fn frame_bound_checked_before_allocation() {
        let mut empty: &[u8] = &[];
        assert!(read_frame(&mut empty, 16).await.unwrap().is_none());
        // A length prefix claiming 4 GiB-1 with no body: rejected on the prefix alone.
        let mut huge: &[u8] = &[0xFF, 0xFF, 0xFF, 0xFF];
        let e = read_frame(&mut huge, MAX_CONTROL_FRAME).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::FrameTooLarge);
        // bound + 1 is rejected, bound exactly is read.
        let msg = ControlMessage::from(Body::Cancel(Cancel {
            request_id: "x".repeat(100),
        }));
        let bytes = encode_frame(&msg);
        let body_len = bytes.len() - 4;
        assert_eq!(
            read_frame(&mut bytes.as_slice(), body_len - 1).await.unwrap_err().code,
            ErrorCode::FrameTooLarge
        );
        assert!(read_frame(&mut bytes.as_slice(), body_len).await.unwrap().is_some());
        // Truncated body is a lost peer, not a panic.
        assert_eq!(
            read_frame(&mut &bytes[..bytes.len() - 1], body_len)
                .await
                .unwrap_err()
                .code,
            ErrorCode::PeerLost
        );
        // write_frame refuses an over-bound frame.
        let big = ControlMessage::from(Body::RemoteChunk(RemoteChunk {
            request_id: String::new(),
            event_json: "y".repeat(MAX_CONTROL_FRAME + 1),
            done: false,
        }));
        let mut sink = Vec::new();
        assert_eq!(
            write_frame(&mut sink, &big).await.unwrap_err().code,
            ErrorCode::FrameTooLarge
        );
        assert!(sink.is_empty());
    }

    #[tokio::test]
    async fn unknown_field_tolerated() {
        // Hello { major = 1 (tag 1 varint), common_version = "x" (tag 3), <tag 99 varint = 7> }
        // encoded by hand; tag 99 wire type 0 → key 792 → varint 0x98 0x06.
        let hello = [0x08, 0x01, 0x1a, 0x01, b'x', 0x98, 0x06, 0x07];
        let mut frame_body = vec![0x0a, hello.len() as u8];
        frame_body.extend_from_slice(&hello);
        let mut frame = (frame_body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&frame_body);
        let msg = read_frame(&mut frame.as_slice(), MAX_CONTROL_FRAME)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            msg.body,
            Some(Body::Hello(Hello {
                major: 1,
                common_version: "x".into(),
                ..Default::default()
            }))
        );
    }

    type Fields = std::collections::BTreeSet<(String, String, u32)>;

    /// `(message, field, tag)` of every prost field above the tests in proto.rs; `Body` variants
    /// belong to `ControlMessage`, named in snake_case as in the .proto.
    fn rust_fields(src: &str) -> Fields {
        let snake = |s: &str| {
            s.chars().enumerate().fold(String::new(), |mut out, (i, c)| {
                if c.is_uppercase() && i > 0 {
                    out.push('_');
                }
                out.push(c.to_ascii_lowercase());
                out
            })
        };
        let (mut msg, mut tag, mut out) = (String::new(), None, Fields::new());
        for line in src.lines().map(str::trim).take_while(|l| *l != "#[cfg(test)]") {
            if let Some(rest) = line.strip_prefix("pub struct ") {
                msg = rest.trim_end_matches(" {").to_string();
            } else if line == "pub enum Body {" {
                msg = "ControlMessage".into();
            } else if let Some((_, rest)) = line.split_once("tag = \"") {
                tag = rest.split('"').next().and_then(|n| n.parse::<u32>().ok());
            } else if let (Some(t), false) = (tag, line.starts_with("//")) {
                let name = match line.strip_prefix("pub ") {
                    Some(field) => field.split(':').next().unwrap_or_default().to_string(),
                    None => snake(line.split('(').next().unwrap_or_default()),
                };
                out.insert((msg.clone(), name, t));
                tag = None;
            }
        }
        out
    }

    /// `(message, field, tag)` and `(message, reserved tag)` of control.proto (one field per line).
    fn proto_fields(src: &str) -> (Fields, Vec<(String, u32)>) {
        let (mut stack, mut fields, mut reserved) = (Vec::<String>::new(), Fields::new(), Vec::new());
        for line in src.lines() {
            let line = line.split("//").next().unwrap_or_default().trim();
            if let Some(rest) = line.strip_prefix("message ") {
                stack.push(rest.trim_end_matches('{').trim().to_string());
            } else if line.starts_with("oneof ") {
                stack.push(stack.last().cloned().unwrap_or_default());
            } else if line == "}" {
                stack.pop();
            } else if let Some(rest) = line.strip_prefix("reserved ") {
                for n in rest.trim_end_matches(';').split(',') {
                    reserved.push((stack.last().cloned().unwrap_or_default(), n.trim().parse().unwrap()));
                }
            } else if let Some((decl, tag)) = line.strip_suffix(';').and_then(|l| l.split_once('=')) {
                if let (Some(msg), Some(name)) = (stack.last(), decl.split_whitespace().last()) {
                    fields.insert((msg.clone(), name.to_string(), tag.trim().parse().unwrap()));
                }
            }
        }
        (fields, reserved)
    }

    #[test]
    fn control_proto_mirrors_proto_rs() {
        let rust = rust_fields(include_str!("proto.rs"));
        let (proto, reserved) = proto_fields(include_str!("../../../proto/common/v1/control.proto"));
        // The parser saw every message (a parser that finds nothing would pass vacuously).
        let messages: std::collections::BTreeSet<&str> = rust.iter().map(|(m, _, _)| m.as_str()).collect();
        assert_eq!(messages.len(), 10, "{messages:?}");
        assert!(rust.contains(&("ControlMessage".into(), "capabilities".into(), 9)));
        assert!(rust.contains(&("SignedCapabilities".into(), "signature".into(), 2)));
        let missing: Vec<_> = rust.difference(&proto).collect();
        let extra: Vec<_> = proto.difference(&rust).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "control.proto out of sync with proto.rs\nmissing from control.proto: {missing:?}\nonly in control.proto: {extra:?}"
        );
        // Retired tags stay reserved and unused.
        assert!(reserved.contains(&("ControlMessage".into(), 2)), "{reserved:?}");
        assert!(reserved.contains(&("Capabilities".into(), 10)), "{reserved:?}");
        for (m, t) in &reserved {
            assert!(
                !rust.iter().any(|(rm, _, rt)| rm == m && rt == t),
                "{m} reuses reserved tag {t}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_oneof_variant_tolerated() {
        // ControlMessage with only field 42 (length-delimited): key 338 → varint 0xd2 0x02.
        let body = [0xd2, 0x02, 0x02, 0x01, 0x00];
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        let msg = read_frame(&mut frame.as_slice(), MAX_CONTROL_FRAME)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(msg.body, None);
    }
}
