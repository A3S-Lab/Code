use std::{io, str::Utf8Error};

use bytes::BytesMut;
use serde_json::Value;
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder};

const HEADER_TERMINATOR: &[u8; 4] = b"\r\n\r\n";
pub(crate) const MAX_HEADER_BYTES: usize = 8 * 1024;
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Errors produced while reading or writing protocol frames.
#[derive(Debug, Error)]
pub(crate) enum LspCodecError {
    #[error("protocol I/O failed: {0}")]
    Io(#[from] io::Error),

    #[error("protocol header exceeds the {max}-byte limit")]
    HeaderTooLarge { max: usize },

    #[error("protocol header is not valid UTF-8: {0}")]
    InvalidHeaderEncoding(#[from] Utf8Error),

    #[error("malformed protocol header line: {line:?}")]
    MalformedHeader { line: String },

    #[error("protocol frame is missing Content-Length")]
    MissingContentLength,

    #[error("protocol frame contains more than one Content-Length header")]
    DuplicateContentLength,

    #[error("invalid Content-Length value: {value:?}")]
    InvalidContentLength { value: String },

    #[error("protocol body length {length} exceeds the {max}-byte limit")]
    BodyTooLarge { length: usize, max: usize },

    #[error("malformed protocol JSON body: {0}")]
    MalformedJson(#[from] serde_json::Error),
}

#[derive(Debug, Default)]
pub(crate) struct LspCodec {
    state: DecodeState,
}

#[derive(Debug, Default)]
enum DecodeState {
    #[default]
    Header,
    Body {
        content_length: usize,
    },
}

impl Decoder for LspCodec {
    type Item = Value;
    type Error = LspCodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.state {
                DecodeState::Header => {
                    let Some(header_end) = find_header_end(src) else {
                        // A terminator arriving after this point would make the
                        // complete header exceed the configured limit.
                        if src.len() >= MAX_HEADER_BYTES {
                            return Err(LspCodecError::HeaderTooLarge {
                                max: MAX_HEADER_BYTES,
                            });
                        }
                        return Ok(None);
                    };

                    let framed_header_len = header_end + HEADER_TERMINATOR.len();
                    if framed_header_len > MAX_HEADER_BYTES {
                        return Err(LspCodecError::HeaderTooLarge {
                            max: MAX_HEADER_BYTES,
                        });
                    }

                    let content_length = parse_content_length(&src[..header_end])?;
                    if content_length > MAX_BODY_BYTES {
                        return Err(LspCodecError::BodyTooLarge {
                            length: content_length,
                            max: MAX_BODY_BYTES,
                        });
                    }

                    let _ = src.split_to(framed_header_len);
                    self.state = DecodeState::Body { content_length };
                }
                DecodeState::Body { content_length } => {
                    if src.len() < content_length {
                        return Ok(None);
                    }

                    let body = src.split_to(content_length);
                    self.state = DecodeState::Header;
                    return serde_json::from_slice(&body)
                        .map(Some)
                        .map_err(LspCodecError::MalformedJson);
                }
            }
        }
    }
}

impl Encoder<Value> for LspCodec {
    type Error = LspCodecError;

    fn encode(&mut self, item: Value, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let body = serde_json::to_vec(&item)?;
        if body.len() > MAX_BODY_BYTES {
            return Err(LspCodecError::BodyTooLarge {
                length: body.len(),
                max: MAX_BODY_BYTES,
            });
        }

        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        dst.reserve(header.len() + body.len());
        dst.extend_from_slice(header.as_bytes());
        dst.extend_from_slice(&body);
        Ok(())
    }
}

fn find_header_end(src: &[u8]) -> Option<usize> {
    src.windows(HEADER_TERMINATOR.len())
        .position(|window| window == HEADER_TERMINATOR)
}

fn parse_content_length(header: &[u8]) -> Result<usize, LspCodecError> {
    let header = std::str::from_utf8(header)?;
    let mut content_length = None;

    for line in header.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            return Err(LspCodecError::MalformedHeader {
                line: line.to_owned(),
            });
        };

        let name = name.trim();
        if name.is_empty() {
            return Err(LspCodecError::MalformedHeader {
                line: line.to_owned(),
            });
        }

        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(LspCodecError::DuplicateContentLength);
            }

            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(LspCodecError::InvalidContentLength {
                    value: value.to_owned(),
                });
            }

            content_length =
                Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| LspCodecError::InvalidContentLength {
                            value: value.to_owned(),
                        })?,
                );
        }
    }

    content_length.ok_or(LspCodecError::MissingContentLength)
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use serde_json::{json, Value};
    use tokio_util::codec::{Decoder, Encoder};

    use super::{LspCodec, LspCodecError, MAX_BODY_BYTES, MAX_HEADER_BYTES};

    fn raw_frame(headers: &str, body: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(headers.len() + 4 + body.len());
        frame.extend_from_slice(headers.as_bytes());
        frame.extend_from_slice(b"\r\n\r\n");
        frame.extend_from_slice(body);
        frame
    }

    #[test]
    fn decodes_fragmented_header_and_body() {
        let body = br#"{"jsonrpc":"2.0","message":"ready"}"#;
        let headers = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8",
            body.len()
        );
        let frame = raw_frame(&headers, body);
        let header_end = frame
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();

        let mut codec = LspCodec::default();
        let mut src = BytesMut::new();

        src.extend_from_slice(&frame[..8]);
        assert!(codec.decode(&mut src).unwrap().is_none());

        src.extend_from_slice(&frame[8..header_end + 6]);
        assert!(codec.decode(&mut src).unwrap().is_none());

        src.extend_from_slice(&frame[header_end + 6..]);
        assert_eq!(
            codec.decode(&mut src).unwrap(),
            Some(json!({"jsonrpc": "2.0", "message": "ready"}))
        );
        assert!(src.is_empty());
    }

    #[test]
    fn decodes_multiple_frames_from_one_buffer() {
        let mut codec = LspCodec::default();
        let mut src = BytesMut::new();
        codec.encode(json!({"id": 1}), &mut src).unwrap();
        codec.encode(json!({"id": 2}), &mut src).unwrap();

        assert_eq!(codec.decode(&mut src).unwrap(), Some(json!({"id": 1})));
        assert_eq!(codec.decode(&mut src).unwrap(), Some(json!({"id": 2})));
        assert!(codec.decode(&mut src).unwrap().is_none());
    }

    #[test]
    fn encoded_content_length_counts_unicode_bytes() {
        let value = json!({"text": "你好 👋"});
        let mut codec = LspCodec::default();
        let mut frame = BytesMut::new();
        codec.encode(value.clone(), &mut frame).unwrap();

        let header_end = frame
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let header = std::str::from_utf8(&frame[..header_end]).unwrap();
        let advertised = header
            .strip_prefix("Content-Length: ")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(advertised, frame.len() - header_end - 4);
        assert_eq!(codec.decode(&mut frame).unwrap(), Some(value));
    }

    #[test]
    fn rejects_oversized_header_and_body() {
        let mut codec = LspCodec::default();
        let mut oversized_header = BytesMut::from(&vec![b'x'; MAX_HEADER_BYTES][..]);
        assert!(matches!(
            codec.decode(&mut oversized_header),
            Err(LspCodecError::HeaderTooLarge {
                max: MAX_HEADER_BYTES
            })
        ));

        let mut codec = LspCodec::default();
        let frame = format!("Content-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        let mut oversized_body = BytesMut::from(frame.as_bytes());
        assert!(matches!(
            codec.decode(&mut oversized_body),
            Err(LspCodecError::BodyTooLarge {
                length,
                max: MAX_BODY_BYTES
            }) if length == MAX_BODY_BYTES + 1
        ));
    }

    #[test]
    fn rejects_missing_duplicate_and_invalid_content_length() {
        let cases = [
            (
                raw_frame("Content-Type: application/json", b"{}"),
                "missing",
            ),
            (
                raw_frame("Content-Length: 2\r\ncontent-length: 2", b"{}"),
                "duplicate",
            ),
            (raw_frame("Content-Length: nope", b"{}"), "invalid"),
        ];

        for (frame, expected) in cases {
            let mut codec = LspCodec::default();
            let mut src = BytesMut::from(frame.as_slice());
            let error = codec.decode(&mut src).unwrap_err();
            match expected {
                "missing" => assert!(matches!(error, LspCodecError::MissingContentLength)),
                "duplicate" => assert!(matches!(error, LspCodecError::DuplicateContentLength)),
                "invalid" => {
                    assert!(matches!(error, LspCodecError::InvalidContentLength { .. }))
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn rejects_malformed_header_and_json() {
        let mut codec = LspCodec::default();
        let mut malformed_header = BytesMut::from(&b"Content-Length 2\r\n\r\n{}"[..]);
        assert!(matches!(
            codec.decode(&mut malformed_header),
            Err(LspCodecError::MalformedHeader { .. })
        ));

        let body = b"{not json}";
        let frame = raw_frame(&format!("Content-Length: {}", body.len()), body);
        let mut codec = LspCodec::default();
        let mut malformed_json = BytesMut::from(frame.as_slice());
        assert!(matches!(
            codec.decode(&mut malformed_json),
            Err(LspCodecError::MalformedJson(_))
        ));
    }

    #[test]
    fn encode_roundtrips_json_values() {
        let values: Vec<Value> = vec![
            json!({"jsonrpc": "2.0", "id": 7, "method": "initialize"}),
            json!([true, null, 42, "text"]),
        ];
        let mut codec = LspCodec::default();
        let mut frame = BytesMut::new();

        for value in &values {
            codec.encode(value.clone(), &mut frame).unwrap();
        }

        for value in values {
            assert_eq!(codec.decode(&mut frame).unwrap(), Some(value));
        }
        assert!(frame.is_empty());
    }

    #[test]
    fn encode_rejects_oversized_json() {
        let value = Value::String("x".repeat(MAX_BODY_BYTES));
        let mut codec = LspCodec::default();
        let mut frame = BytesMut::new();

        assert!(matches!(
            codec.encode(value, &mut frame),
            Err(LspCodecError::BodyTooLarge {
                length,
                max: MAX_BODY_BYTES
            }) if length == MAX_BODY_BYTES + 2
        ));
        assert!(frame.is_empty());
    }
}
