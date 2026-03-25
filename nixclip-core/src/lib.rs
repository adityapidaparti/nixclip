pub mod config;
pub mod error;
pub mod ipc;
pub mod pipeline;
pub mod search;
pub mod storage;

pub use error::{NixClipError, Result};

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// EntryId
// ---------------------------------------------------------------------------

pub type EntryId = i64;

// ---------------------------------------------------------------------------
// ContentClass
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentClass {
    Text,
    RichText,
    Image,
    Files,
    Url,
}

impl ContentClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentClass::Text => "text",
            ContentClass::RichText => "richtext",
            ContentClass::Image => "image",
            ContentClass::Files => "files",
            ContentClass::Url => "url",
        }
    }
}

impl fmt::Display for ContentClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ContentClass {
    type Err = NixClipError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "text" => Ok(ContentClass::Text),
            "richtext" => Ok(ContentClass::RichText),
            "image" => Ok(ContentClass::Image),
            "files" => Ok(ContentClass::Files),
            "url" => Ok(ContentClass::Url),
            other => Err(NixClipError::Pipeline(format!(
                "unknown content class: {other}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// RestoreMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RestoreMode {
    Original,
    PlainText,
}

// ---------------------------------------------------------------------------
// MimePayload
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MimePayload {
    pub mime: String,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// NewEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEntry {
    pub content_class: ContentClass,
    pub preview_text: Option<String>,
    pub canonical_hash: [u8; 32],
    pub representations: Vec<MimePayload>,
    pub source_app: Option<String>,
    pub ephemeral: bool,
}

// ---------------------------------------------------------------------------
// EntrySummary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntrySummary {
    pub id: EntryId,
    /// Unix timestamp in milliseconds.
    pub created_at: i64,
    /// Unix timestamp in milliseconds.
    pub last_seen_at: i64,
    pub pinned: bool,
    pub ephemeral: bool,
    pub content_class: ContentClass,
    pub preview_text: Option<String>,
    pub source_app: Option<String>,
    pub thumbnail: Option<Vec<u8>>,
    /// Byte ranges in `preview_text` that matched the search query.
    /// Each tuple is (start_byte, length_bytes). Empty when there is no query.
    #[serde(default)]
    pub match_ranges: Vec<(u32, u32)>,
}

// ---------------------------------------------------------------------------
// Representation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Representation {
    pub mime: String,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Query / QueryResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub text: Option<String>,
    pub content_class: Option<ContentClass>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub entries: Vec<EntrySummary>,
    pub total: u32,
}

// ---------------------------------------------------------------------------
// PruneStats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PruneStats {
    pub entries_deleted: u32,
    pub blobs_deleted: u32,
    pub bytes_freed: u64,
}

// ---------------------------------------------------------------------------
// ProcessedEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ProcessedEntry {
    pub content_class: ContentClass,
    pub preview_text: Option<String>,
    pub canonical_hash: [u8; 32],
    pub representations: Vec<MimePayload>,
    pub thumbnail: Option<Vec<u8>>,
    pub metadata: EntryMetadata,
}

// ---------------------------------------------------------------------------
// EntryMetadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct EntryMetadata {
    pub image_dimensions: Option<(u32, u32)>,
    pub file_count: Option<usize>,
    pub url_domain: Option<String>,
}

// ---------------------------------------------------------------------------
// StoreStats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreStats {
    pub entry_count: u64,
    pub blob_size_bytes: u64,
    pub db_size_bytes: u64,
}
