use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sha2::{Digest, Sha256};

use crate::tines::TinesError;

pub const MAX_CONTENT_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_CONTENT_CHUNK_BYTES: usize = 32 * 1024;

pub struct BoundedContent {
    pub encoding: &'static str,
    pub total_bytes: usize,
    pub sha256: String,
    pub offset: usize,
    pub returned_length: usize,
    pub content: String,
    pub next_offset: Option<usize>,
}

/// Digests a complete bounded source and returns a UTF-8-safe or base64 slice.
/// The caller resolves and verifies source identity around the byte fetch.
pub fn bounded_content(
    bytes: &[u8],
    text_source: bool,
    mime_type: Option<&str>,
    offset: u64,
    chunk_bytes: u32,
    expected_sha256: Option<&str>,
    status: u16,
) -> Result<BoundedContent, TinesError> {
    if bytes.len() > MAX_CONTENT_SOURCE_BYTES {
        return Err(TinesError::Schema {
            status: Some(status),
            code: "content_source_too_large",
            message: "Tines content exceeded the 1 MiB source limit.",
        });
    }
    let chunk_bytes = chunk_bytes as usize;
    if !(4..=MAX_CONTENT_CHUNK_BYTES).contains(&chunk_bytes) {
        return Err(TinesError::Validation {
            field: "chunk_bytes",
            message: "must be between 4 and 32768",
        });
    }
    let offset = usize::try_from(offset).map_err(|_| TinesError::Validation {
        field: "offset",
        message: "is outside the supported range",
    })?;
    if offset > bytes.len() {
        return Err(TinesError::Validation {
            field: "offset",
            message: "must not exceed the source byte length",
        });
    }

    let sha256 = sha256_hex(bytes);
    if expected_sha256.is_some_and(|expected| !expected.eq_ignore_ascii_case(&sha256)) {
        return Err(content_changed());
    }

    let is_text = text_source || is_text_mime_type(mime_type);
    let (encoding, content, returned_length, next_offset) = if is_text {
        let text = std::str::from_utf8(bytes).map_err(|_| TinesError::Schema {
            status: Some(status),
            code: "invalid_text_content",
            message: "Tines declared content as text, but it was not valid UTF-8.",
        })?;
        if !text.is_char_boundary(offset) {
            return Err(TinesError::Validation {
                field: "offset",
                message: "must be a UTF-8 character boundary for text content",
            });
        }
        let end = text_chunk_end(text, offset, chunk_bytes);
        let content = text[offset..end].to_owned();
        (
            "utf-8",
            content,
            end - offset,
            (end < bytes.len()).then_some(end),
        )
    } else {
        let end = offset.saturating_add(chunk_bytes).min(bytes.len());
        (
            "base64",
            BASE64.encode(&bytes[offset..end]),
            end - offset,
            (end < bytes.len()).then_some(end),
        )
    };

    Ok(BoundedContent {
        encoding,
        total_bytes: bytes.len(),
        sha256,
        offset,
        returned_length,
        content,
        next_offset,
    })
}

fn content_changed() -> TinesError {
    TinesError::Schema {
        status: None,
        code: "content_changed",
        message: "The content or its digest changed between bounded reads; restart at offset 0.",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_text_mime_type(mime_type: Option<&str>) -> bool {
    let Some(mime_type) = mime_type else {
        return false;
    };
    let media_type = mime_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    media_type.starts_with("text/")
        || matches!(
            media_type.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
        )
        || media_type.ends_with("+json")
        || media_type.ends_with("+xml")
}

fn text_chunk_end(text: &str, offset: usize, max_bytes: usize) -> usize {
    let target = offset.saturating_add(max_bytes).min(text.len());
    if text.is_char_boundary(target) {
        return target;
    }
    let mut end = target;
    while end > offset && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end > offset {
        return end;
    }
    let mut end = target;
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::bounded_content;

    #[test]
    fn bounded_text_chunks_end_before_an_incomplete_utf8_sequence() {
        let mut bytes = vec![b'a'; 32767];
        bytes.extend_from_slice("☃z".as_bytes());
        let chunk = bounded_content(&bytes, true, Some("text/plain"), 0, 32768, None, 200)
            .expect("the first text chunk should be valid");
        assert_eq!(chunk.returned_length, 32767);
        assert_eq!(chunk.next_offset, Some(32767));
        assert_eq!(chunk.content, "a".repeat(32767));
    }

    #[test]
    fn bounded_binary_chunks_are_base64_and_check_the_whole_source_digest() {
        let bytes = [0, 1, 2, 253, 254, 255];
        let first = bounded_content(
            &bytes,
            false,
            Some("application/octet-stream"),
            0,
            4,
            None,
            200,
        )
        .expect("binary content should be encoded");
        assert_eq!(first.encoding, "base64");
        assert_eq!(first.returned_length, 4);
        assert_eq!(first.next_offset, Some(4));
        assert!(
            bounded_content(
                &bytes,
                false,
                Some("application/octet-stream"),
                4,
                4,
                Some(&"0".repeat(64)),
                200,
            )
            .is_err()
        );
    }
}
