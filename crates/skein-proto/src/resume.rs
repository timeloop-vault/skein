//! Wire DTOs of the resume probes.

/// A Hermes harness still waiting for its session id, as the frontend
/// sends it.
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingDto {
    pub harness_id: String,
    pub spawned_at_ms: i64,
}

/// Size and mtime of a Claude transcript, for the #423 supervisor's
/// "adapter silent while the file keeps growing" check.
#[derive(serde::Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptStatDto {
    pub size: u64,
    pub mtime_ms: u64,
}
