//! Shared HTTP response helpers for providers.

/// Default cap for reading an HTTP **error**-response body. Error bodies are
/// only used for diagnostics (and truncated again by
/// [`truncate_error_body`](crate::truncate_error_body)), so a few KiB is
/// plenty — reading more just risks `OOM`ing the process on a hostile or
/// compromised endpoint that returns a huge body.
pub const MAX_ERROR_BODY_READ_BYTES: usize = 2048;

/// Default cap for reading a **success**-response body that is returned to the
/// caller (e.g. a webhook's response payload). Generous enough for any
/// legitimate ack, but bounded so a hostile or user-controlled endpoint can't
/// OOM the gateway by replying `200` with a giant body.
pub const MAX_RESPONSE_BODY_READ_BYTES: usize = 1_048_576;

/// Read at most `max_bytes` of a [`reqwest::Response`] body and return it as a
/// lossy UTF-8 string.
///
/// Unlike [`reqwest::Response::text`], which reads the **entire** body into
/// memory before returning, this pumps [`reqwest::Response::chunk`] in a loop
/// and stops as soon as the byte cap is reached. A response whose
/// `Content-Length` is huge (or unbounded/chunked) is truncated at the cap
/// rather than read fully — so an action's target endpoint cannot OOM the
/// gateway by replying with a giant body.
pub async fn read_bounded_body(mut response: reqwest::Response, max_bytes: usize) -> String {
    let mut buf: Vec<u8> = Vec::with_capacity(max_bytes.min(1024));
    while buf.len() < max_bytes {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = max_bytes - buf.len();
                let take = chunk.len().min(remaining);
                buf.extend_from_slice(&chunk[..take]);
                if chunk.len() > remaining {
                    // Hit the cap mid-chunk — drop the rest and stop pulling.
                    break;
                }
            }
            // End of stream or transport error — return what we have.
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}
