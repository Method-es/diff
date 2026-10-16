//! The Language Server Protocol's base protocol: each message is a header part (`Content-Length:
//! <n>`, optionally `Content-Type`, each line ending in `\r\n`), an empty line, then `n` bytes of
//! UTF-8 JSON (docs/PLAN.md §18.4).
//!
//! [`Decoder`] takes a server's stdout in whatever chunks the pipe gives (a message split
//! anywhere, a header split across chunks, several messages in one chunk) and hands back whole
//! message bodies. [`encode`] frames one body.

/// The largest body dv accepts. A language server's biggest answers (references across a large
/// workspace) are a few megabytes; a length beyond this is a broken stream.
pub const MAX_BODY: usize = 64 * 1024 * 1024;

/// The longest header part dv reads before deciding the stream is broken.
const MAX_HEADER: usize = 8 * 1024;

/// `body` framed with its `Content-Length` header.
pub fn encode(body: &[u8]) -> Vec<u8> {
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body);
    frame
}

/// Why a stream stopped making sense. The decoder drops what it can't read and looks for the
/// next header, so a server that printed a stray line to stdout loses only that.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("a header without Content-Length: {0:?}")]
    NoLength(String),
    #[error("a message of {0} bytes, more than dv reads")]
    TooLarge(usize),
    #[error("a header part longer than {MAX_HEADER} bytes")]
    HeaderTooLong,
}

/// The header name, matched without regard to case.
const LENGTH: &[u8] = b"content-length";

/// Reassembles message bodies from a byte stream.
///
/// A header part is read line by line (lines end in `\n`, a `\r` before it dropped), and its
/// `Content-Length` is found wherever it is in the part: a server that printed a stray line
/// (`debug: starting\n`) just before a header loses only that line, never the message after
/// it.
#[derive(Debug)]
pub struct Decoder {
    buffer: Vec<u8>,
    /// The body length of the message being read, once its header has been read.
    body_len: Option<usize>,
    /// Bytes of a refused (too large) body still to drop as they arrive.
    skip: usize,
    /// [`MAX_BODY`]; smaller in tests.
    max_body: usize,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            buffer: Vec::new(),
            body_len: None,
            skip: 0,
            max_body: MAX_BODY,
        }
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends the next bytes of the stream.
    pub fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// The next whole message body, `None` until one has fully arrived. An error drops what it
    /// can't read (a header part without a length, and anything up to the next
    /// `Content-Length`; a body over [`MAX_BODY`], exactly its length, reported once); calling
    /// again carries on.
    pub fn next_message(&mut self) -> Option<Result<Vec<u8>, FrameError>> {
        loop {
            if self.skip > 0 {
                let dropped = self.skip.min(self.buffer.len());
                self.buffer.drain(..dropped);
                self.skip -= dropped;
                if self.skip > 0 {
                    return None;
                }
            }
            if let Some(len) = self.body_len {
                if self.buffer.len() < len {
                    return None;
                }
                let body: Vec<u8> = self.buffer.drain(..len).collect();
                self.body_len = None;
                return Some(Ok(body));
            }
            let Some(end) = find(&self.buffer, b"\r\n\r\n") else {
                if self.buffer.len() > MAX_HEADER {
                    self.drop_garbage();
                    return Some(Err(FrameError::HeaderTooLong));
                }
                return None;
            };
            let header = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
            self.buffer.drain(..end + 4);
            match content_length(&header) {
                Some(len) if len > self.max_body => {
                    self.skip = len;
                    return Some(Err(FrameError::TooLarge(len)));
                }
                Some(len) => self.body_len = Some(len),
                None => {
                    let next = find_length(&self.buffer).unwrap_or(self.buffer.len());
                    self.buffer.drain(..next);
                    return Some(Err(FrameError::NoLength(header)));
                }
            }
        }
    }

    /// A header part grew past [`MAX_HEADER`] without ending: drops it up to the last
    /// `Content-Length` in it, or all but a tail that may be the start of one.
    fn drop_garbage(&mut self) {
        let keep_from = match rfind_length(&self.buffer) {
            Some(at) if at > 0 => at,
            _ => self.buffer.len().saturating_sub(LENGTH.len()),
        };
        self.buffer.drain(..keep_from);
    }
}

/// The `Content-Length` of a header part: on any of its lines, after anything a stray print
/// left in front of it, its name in any case.
fn content_length(header: &str) -> Option<usize> {
    header.split('\n').find_map(|line| {
        let line = line.trim_end_matches('\r');
        let at = find_length(line.as_bytes())?;
        let (_, value) = line[at + LENGTH.len()..].split_once(':')?;
        value.trim().parse().ok()
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Where `Content-Length` (any case) first starts in `bytes`.
fn find_length(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(LENGTH.len())
        .position(|window| window.eq_ignore_ascii_case(LENGTH))
}

/// Where `Content-Length` (any case) last starts in `bytes`.
fn rfind_length(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(LENGTH.len())
        .rposition(|window| window.eq_ignore_ascii_case(LENGTH))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(decoder: &mut Decoder) -> Vec<Result<String, FrameError>> {
        std::iter::from_fn(|| decoder.next_message())
            .map(|result| result.map(|body| String::from_utf8(body).unwrap()))
            .collect()
    }

    #[test]
    fn a_frame_round_trips() {
        let mut decoder = Decoder::new();
        decoder.push(&encode(br#"{"id":1}"#));
        assert_eq!(drain(&mut decoder), vec![Ok(r#"{"id":1}"#.to_string())]);
    }

    #[test]
    fn several_messages_in_one_chunk() {
        let mut stream = encode(b"one");
        stream.extend(encode(b"two"));
        stream.extend(encode("thr\u{e9}e".as_bytes()));
        let mut decoder = Decoder::new();
        decoder.push(&stream);
        assert_eq!(
            drain(&mut decoder),
            vec![
                Ok("one".to_string()),
                Ok("two".to_string()),
                Ok("thr\u{e9}e".to_string())
            ]
        );
    }

    #[test]
    fn a_message_split_at_every_byte() {
        let mut stream = encode("{\"text\":\"\u{1f600} wide\"}".as_bytes());
        stream.extend(encode(b"second"));
        let mut decoder = Decoder::new();
        let mut bodies = Vec::new();
        for byte in &stream {
            decoder.push(std::slice::from_ref(byte));
            bodies.extend(drain(&mut decoder));
        }
        assert_eq!(
            bodies,
            vec![
                Ok("{\"text\":\"\u{1f600} wide\"}".to_string()),
                Ok("second".to_string())
            ]
        );
    }

    #[test]
    fn a_header_split_across_chunks_with_a_content_type() {
        let mut decoder = Decoder::new();
        decoder.push(
            b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r",
        );
        assert!(decoder.next_message().is_none());
        decoder.push(b"\n\r\nok");
        assert_eq!(drain(&mut decoder), vec![Ok("ok".to_string())]);
    }

    #[test]
    fn a_stray_header_part_is_dropped_and_the_next_message_read() {
        let mut stream = b"Listening on stdio\r\n\r\n".to_vec();
        stream.extend(encode(b"after"));
        let mut decoder = Decoder::new();
        decoder.push(&stream);
        let results = drain(&mut decoder);
        assert!(matches!(results[0], Err(FrameError::NoLength(_))));
        assert_eq!(results[1..], [Ok("after".to_string())]);
    }

    #[test]
    fn a_stray_line_before_a_header_loses_nothing() {
        let mut stream = b"debug: starting\n".to_vec();
        stream.extend(encode(b"first"));
        stream.extend(b"oops\n");
        stream.extend(encode(b"second"));
        // No newline between the stray print and the header at all.
        stream.extend(b"tail");
        stream.extend(encode(b"third"));
        let mut decoder = Decoder::new();
        decoder.push(&stream);
        assert_eq!(
            drain(&mut decoder),
            vec![
                Ok("first".to_string()),
                Ok("second".to_string()),
                Ok("third".to_string())
            ]
        );
    }

    #[test]
    fn resync_finds_the_next_header_in_any_case() {
        let mut decoder = Decoder::new();
        decoder.push(b"junk\r\n\r\nmore junk content-length: 2\r\n\r\nok");
        let results = drain(&mut decoder);
        assert!(matches!(results[0], Err(FrameError::NoLength(_))));
        assert_eq!(results[1..], [Ok("ok".to_string())]);
    }

    #[test]
    fn a_body_over_the_cap_is_skipped_exactly_with_one_error() {
        let mut decoder = Decoder {
            max_body: 10,
            ..Decoder::new()
        };
        decoder.push(b"Content-Length: 20\r\n\r\n0123456");
        let mut results = drain(&mut decoder);
        decoder.push(b"789012345");
        results.extend(drain(&mut decoder));
        let mut rest = b"6789".to_vec();
        rest.extend(encode(b"next"));
        decoder.push(&rest);
        results.extend(drain(&mut decoder));
        assert_eq!(
            results,
            vec![Err(FrameError::TooLarge(20)), Ok("next".to_string())]
        );
    }

    #[test]
    fn a_header_that_never_ends_is_dropped_once() {
        let mut decoder = Decoder::new();
        decoder.push(&vec![b'x'; MAX_HEADER + 100]);
        let mut results = drain(&mut decoder);
        decoder.push(&encode(b"after"));
        results.extend(drain(&mut decoder));
        assert_eq!(
            results,
            vec![Err(FrameError::HeaderTooLong), Ok("after".to_string())]
        );
    }
}
