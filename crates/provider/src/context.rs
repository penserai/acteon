use acteon_core::ResolvedAttachment;

/// Additional context passed to providers during dispatch.
///
/// Contains resolved attachment data that providers can use if they
/// support file attachments. Providers that don't support attachments
/// can ignore this context entirely.
#[derive(Debug, Default)]
pub struct DispatchContext {
    /// Resolved file attachments (decoded from `base64`).
    pub attachments: Vec<ResolvedAttachment>,
}
