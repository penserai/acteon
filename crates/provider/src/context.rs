use acteon_blob::ResolvedBlob;

/// Additional context passed to providers during dispatch.
///
/// Contains resolved attachment data that providers can use if they
/// support file attachments. Providers that don't support attachments
/// can ignore this context entirely.
#[derive(Debug, Default)]
pub struct DispatchContext {
    /// Resolved file attachments (blob references fetched, inline data decoded).
    pub attachments: Vec<ResolvedBlob>,
}
