//! Server-Sent Events parsing helpers.

/// Incrementally decode UTF-8 without replacing a code point split across
/// transport chunks.
///
/// HTTP byte streams make no promise that a chunk ends on a UTF-8 boundary.
/// Retaining the incomplete suffix keeps SSE JSON lossless while still
/// rejecting genuinely invalid byte sequences.
#[derive(Debug, Default)]
pub(crate) struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub(crate) fn push_to(
        &mut self,
        chunk: &[u8],
        output: &mut String,
    ) -> Result<(), std::str::Utf8Error> {
        self.pending.extend_from_slice(chunk);
        match std::str::from_utf8(&self.pending) {
            Ok(text) => {
                output.push_str(text);
                self.pending.clear();
                Ok(())
            }
            Err(error) => {
                let valid_up_to = error.valid_up_to();
                if valid_up_to > 0 {
                    let text = std::str::from_utf8(&self.pending[..valid_up_to])
                        .expect("UTF-8 error valid prefix");
                    output.push_str(text);
                    self.pending.drain(..valid_up_to);
                }
                if error.error_len().is_some() {
                    Err(error)
                } else {
                    Ok(())
                }
            }
        }
    }

    pub(crate) fn finish(&self) -> Result<(), std::str::Utf8Error> {
        std::str::from_utf8(&self.pending).map(|_| ())
    }
}

/// Remove every complete SSE event from the front of `buffer`.
///
/// A blank line ends an event. The line ending may be LF, CRLF, or CR, so a
/// spec-shaped frame `data: ...\r\n\r\n` is one event. Searching for the two
/// bytes `\n\n` misses that frame: the newlines are separated by `\r`.
///
/// A trailing CR stays in `buffer` unless `eof` is set, because the next chunk
/// may complete a CRLF pair. Callers pass `eof` only after the stream has
/// ended.
pub(crate) fn take_sse_events(buffer: &mut String, eof: bool) -> Vec<String> {
    let mut events = Vec::new();
    while let Some((event, consumed)) = next_sse_event(buffer, false) {
        let event = event.to_string();
        buffer.drain(..consumed);
        if !event.is_empty() {
            events.push(event);
        }
    }
    if eof {
        if let Some((event, consumed)) = next_sse_event(buffer, true) {
            let event = event.to_string();
            buffer.drain(..consumed);
            if !event.is_empty() {
                events.push(event);
            }
        }
    }
    events
}

/// One complete event and the byte index just past its blank-line delimiter.
fn next_sse_event(buffer: &str, eof: bool) -> Option<(&str, usize)> {
    let bytes = buffer.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let line_start = index;
        while index < bytes.len() && bytes[index] != b'\n' && bytes[index] != b'\r' {
            index += 1;
        }
        if index == bytes.len() {
            return None;
        }
        let ending_len = if bytes[index] == b'\r' {
            if index + 1 == bytes.len() {
                if !eof {
                    return None;
                }
                1
            } else if bytes[index + 1] == b'\n' {
                2
            } else {
                1
            }
        } else {
            1
        };
        let blank = index == line_start;
        index += ending_len;
        if blank {
            return Some((&buffer[..line_start], index));
        }
    }
    None
}

/// Return the value of an SSE `data:` field.
///
/// The SSE field grammar allows `data:<value>` and `data: <value>`. When a
/// single space follows the colon, it is ignored by the parser.
pub(crate) fn data_field_value(line: &str) -> Option<&str> {
    let value = line.strip_prefix("data:")?;
    Some(value.strip_prefix(' ').unwrap_or(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_sse_events_accepts_crlf_tools_list() {
        let frame = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[{\"name\":\"lookup\"}]}}\r\n\r\n";
        let mut buffer = frame.to_string();
        let events = take_sse_events(&mut buffer, false);
        assert!(buffer.is_empty(), "the CRLF frame is one complete event");
        assert_eq!(events.len(), 1);
        let mut data = String::new();
        for line in events[0].lines() {
            if let Some(value) = data_field_value(line) {
                data.push_str(value);
            }
        }
        assert_eq!(
            data,
            "{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"tools\":[{\"name\":\"lookup\"}]}}"
        );
        assert!(!data.contains('\r'));
    }

    #[test]
    fn take_sse_events_keeps_lf_frames_and_a_split_crlf_pair() {
        let mut buffer = "data: {\"ok\":true}\n\n".to_string();
        assert_eq!(take_sse_events(&mut buffer, false).len(), 1);
        assert!(buffer.is_empty());

        let mut split = "data: {\"ok\":true}\r".to_string();
        assert!(
            take_sse_events(&mut split, false).is_empty(),
            "a trailing CR may be the start of CRLF"
        );
        assert!(split.ends_with('\r'));
        split.push_str("\n\r\n");
        let events = take_sse_events(&mut split, false);
        assert_eq!(events.len(), 1);
        assert!(split.is_empty());
        assert_eq!(
            events[0].lines().find_map(data_field_value),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn take_sse_events_skips_a_crlf_blank_line_without_stalling() {
        let mut buffer = "\r\n\r\ndata: {\"ok\":true}\r\n\r\n".to_string();
        let events = take_sse_events(&mut buffer, false);
        assert_eq!(events.len(), 1);
        assert!(buffer.is_empty());
    }

    #[test]
    fn data_field_value_accepts_optional_space() {
        assert_eq!(
            data_field_value("data: {\"ok\":true}"),
            Some("{\"ok\":true}")
        );
        assert_eq!(
            data_field_value("data:{\"ok\":true}"),
            Some("{\"ok\":true}")
        );
        assert_eq!(data_field_value("data: [DONE]"), Some("[DONE]"));
        assert_eq!(data_field_value("data:[DONE]"), Some("[DONE]"));
    }

    #[test]
    fn data_field_value_ignores_non_data_fields() {
        assert_eq!(data_field_value("event: message"), None);
        assert_eq!(data_field_value("id: 1"), None);
    }

    #[test]
    fn utf8_stream_decoder_preserves_code_points_split_at_every_byte() {
        let input = "data: {\"title\":\"维护治理\"}\n\n";
        let mut decoder = Utf8StreamDecoder::default();
        let mut output = String::new();

        for byte in input.as_bytes() {
            decoder.push_to(&[*byte], &mut output).unwrap();
        }

        decoder.finish().unwrap();
        assert_eq!(output, input);
        assert!(!output.contains('\u{fffd}'));
    }

    #[test]
    fn utf8_stream_decoder_rejects_invalid_bytes() {
        let mut decoder = Utf8StreamDecoder::default();
        assert!(decoder.push_to(&[0xff], &mut String::new()).is_err());
    }
}
