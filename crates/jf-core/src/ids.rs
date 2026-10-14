//! Jellyfin-compatible identifier handling.
//!
//! Jellyfin serializes item ids as 32-character lowercase hex with no dashes
//! (a .NET `Guid` in `ToString("N")` form). Plugins and clients rely on that
//! shape, so ids are generated and parsed to match exactly.

use std::fmt;

/// Error returned when a string is not a valid Jellyfin item id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdParseError(pub String);

impl fmt::Display for IdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a valid item id (expected 32 lowercase hex chars)", self.0)
    }
}

impl std::error::Error for IdParseError {}

/// Generate a new random item id in Jellyfin's dash-less hex form.
pub fn new_item_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Validate and normalize a string into an item id.
///
/// Uppercase is accepted on input and folded to lowercase, since clients vary.
pub fn item_id(raw: &str) -> Result<String, IdParseError> {
    if raw.len() != 32 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(IdParseError(raw.to_owned()));
    }
    Ok(raw.to_ascii_lowercase())
}