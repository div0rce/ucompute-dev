//! Minimal Server-Sent-Events framing: events are separated by a blank line, payload lines start
//! with `data:` (llama-server also emits `error:` lines for mid-stream failures). Chunks may split
//! anywhere, including inside a UTF-8 sequence, so the buffer is bytes until an event is complete.
//! ponytail: LF-only framing (what llama-server sends); add CRLF when another backend needs it.

#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    /// Feed bytes; returns the payload of every event completed so far, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let event: Vec<u8> = self.buf.drain(..end + 2).collect();
            if let Some(p) = payload(&event) {
                out.push(p);
            }
        }
        out
    }
}

fn payload(event: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(event);
    let data: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            let d = line.strip_prefix("data:").or_else(|| line.strip_prefix("error:"))?;
            Some(d.strip_prefix(' ').unwrap_or(d))
        })
        .collect();
    (!data.is_empty()).then(|| data.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const COMPLETION_FIXTURE: &str = include_str!("fixtures/completion_stream.sse");
    pub const CHAT_FIXTURE: &str = include_str!("fixtures/chat_stream.sse");

    fn parse_in_chunks(text: &str, size: usize) -> Vec<String> {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        for c in text.as_bytes().chunks(size) {
            out.extend(p.push(c));
        }
        out
    }

    #[test]
    fn recorded_completion_stream_has_33_events_at_any_chunking() {
        let whole = parse_in_chunks(COMPLETION_FIXTURE, COMPLETION_FIXTURE.len());
        assert_eq!(whole.len(), 33, "32 tokens + final stop chunk");
        for size in [1, 2, 3, 7, 64, 1000] {
            assert_eq!(parse_in_chunks(COMPLETION_FIXTURE, size), whole, "chunk size {size}");
        }
        let first: serde_json::Value = serde_json::from_str(&whole[0]).unwrap();
        assert_eq!(first["content"], " named");
        assert_eq!(first["tokens"][0], 4257);
        let last: serde_json::Value = serde_json::from_str(&whole[32]).unwrap();
        assert_eq!(last["stop"], true);
        assert_eq!(last["stop_type"], "limit");
    }

    #[test]
    fn recorded_chat_stream_ends_with_done_marker() {
        let evs = parse_in_chunks(CHAT_FIXTURE, 5);
        assert_eq!(evs.len(), 12);
        assert_eq!(evs.last().unwrap(), "[DONE]");
        let usage: serde_json::Value = serde_json::from_str(&evs[10]).unwrap();
        assert_eq!(usage["usage"]["completion_tokens"], 8);
        assert!(usage["timings"]["predicted_per_second"].is_number());
    }

    #[test]
    fn partial_event_waits_and_multiline_data_joins() {
        let mut p = SseParser::default();
        assert!(p.push(b"data: {\"a\":1}").is_empty());
        assert!(p.push(b"\n").is_empty());
        assert_eq!(
            p.push(b"\ndata: x\r\ndata: y\r\n\r\n\n"),
            vec!["{\"a\":1}".to_string(), "x\ny".into()]
        );
        assert_eq!(
            p.push(b"error: {\"code\":500}\n\n: comment\n\n"),
            vec!["{\"code\":500}".to_string()]
        );
        let split_utf8 = "data: \u{e9}\n\n".as_bytes();
        assert!(p.push(&split_utf8[..7]).is_empty());
        assert_eq!(p.push(&split_utf8[7..]), vec!["\u{e9}".to_string()]);
    }
}
