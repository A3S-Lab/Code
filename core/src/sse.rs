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
