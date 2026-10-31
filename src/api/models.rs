use serde::{Deserialize, Serialize};

/// Human quality labels mapped onto a numeric `target_kbps` ceiling (G2). The
/// labels are the stable front-end contract; the ceilings are retunnable. The
/// server-authoritative primitive is the kbps number — front-ends may send
/// either. Ladder (kbps): lowest 16, low 48, medium 128, high 256, veryhigh 512,
/// extreme 1024, best/lossless 0 (no ceiling). `lossless` shares best's ceiling:
/// the server pass-throughs and never re-encodes, and `PlayerStatus.codec`
/// reports the encoding actually delivered (so a lossy source is visible).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Lowest,
    Low,
    Medium,
    High,
    Veryhigh,
    Extreme,
    Best,
    Lossless,
}

impl Quality {
    /// Ceiling in kbps for this label. `0` = no ceiling (best/lossless =
    /// source-max; lossless additionally demands bit-exact pass-through).
    pub fn ceiling_kbps(self) -> u32 {
        match self {
            Quality::Lowest => 16,
            Quality::Low => 48,
            Quality::Medium => 128,
            Quality::High => 256,
            Quality::Veryhigh => 512,
            Quality::Extreme => 1024,
            Quality::Best => 0,
            Quality::Lossless => 0,
        }
    }
}

/// Tolerant deserializer for `Option<Quality>`: an absent field is `None`
/// ([`serde(default)]`), and an unrecognized string is also `None` so an
/// evolving front-end label never 422s the request — it falls back to the
/// server default / best, exactly as the documented contract states.
fn deserialize_quality_opt<'de, D>(deserializer: D) -> Result<Option<Quality>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(deserializer)?;
    let label = match raw {
        None => return Ok(None),
        Some(s) => s.trim().to_lowercase(),
    };
    let q = match label.as_str() {
        "lowest" => Quality::Lowest,
        "low" => Quality::Low,
        "medium" => Quality::Medium,
        "high" => Quality::High,
        "veryhigh" => Quality::Veryhigh,
        "extreme" => Quality::Extreme,
        "best" => Quality::Best,
        "lossless" => Quality::Lossless,
        _ => return Ok(None),
    };
    Ok(Some(q))
}

/// Resolve the effective bandwidth ceiling (kbps) for a request. `0`/absent =
/// best (no ceiling). Precedence: explicit `target_kbps > 0` wins; else a
/// `quality` label through the ladder; else the server `default_target_kbps`;
/// else best (0). An unknown/absent `quality` never errors — it is skipped.
pub fn resolve_ceiling(target_kbps: Option<u32>, quality: Option<Quality>, server_default: u32) -> u32 {
    if let Some(kbps) = target_kbps {
        if kbps > 0 {
            return kbps;
        }
    }
    if let Some(q) = quality {
        return q.ceiling_kbps();
    }
    server_default
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub track_id: String,
    pub stream_url: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<u64>,
    /// Requested bandwidth ceiling in kbps. `Some(0)` and `None` == best (no
    /// ceiling); a `Some(n > 0)` value is authoritative over `quality`.
    #[serde(default)]
    pub target_kbps: Option<u32>,
    /// Sugar label for a preset ceiling (see [`Quality`]). Lower precedence
    /// than an explicit `target_kbps`. Unknown labels are treated as absent
    /// (fall back to the server default / best) rather than rejected.
    #[serde(default, deserialize_with = "deserialize_quality_opt")]
    pub quality: Option<Quality>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SpeedPayload {
    pub speed: f32,
    #[serde(default)]
    pub pitch: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VolumePayload {
    pub volume: f32,
}

/// Runtime setter for the default bandwidth ceiling, so a front-end can change
/// it without a restart (G2). `target_kbps = 0` (or null) resets to best.
#[derive(Debug, Serialize, Deserialize)]
pub struct TargetPayload {
    #[serde(default)]
    pub target_kbps: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_quality_opt")]
    pub quality: Option<Quality>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SeekPayload {
    pub position: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PoolSettings {
    pub max_disk_pool_bytes: u64,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PlayerStateLabel {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub state: PlayerStateLabel,
    pub volume: f32,
    pub speed: f32,
    pub pitch: f32,
    pub current_track: Option<Track>,
    pub disk_pool_utilization_bytes: u64,
    pub next_cached: bool,
    pub prev_cached: bool,
    pub position_sec: Option<u64>,
    pub duration_sec: Option<u64>,
    /// Human playhead for progress-bar front-ends: `"m:ss/m:ss"`
    /// (`position/duration`) once both are known, otherwise the position alone.
    /// A front-end reading only `position_sec`/`duration_sec` ints has to build
    /// this itself, and one that only reads `time` gets no progress at all.
    #[serde(default)]
    pub time: Option<String>,
    pub audio_available: bool,
    /// The bandwidth ceiling (kbps) applied to the current/last track. `0` =
    /// best (no ceiling).
    #[serde(default)]
    pub target_kbps: u32,
    /// The delivered bitrate when known (e.g. the selected rendition's average
    /// bitrate), so a front-end can show reality even when a ceiling couldn't
    /// be met. `None` when unknown.
    #[serde(default)]
    pub actual_kbps: Option<u32>,
    /// Short codec label for the current/last track (e.g. "opus", "aac"), when
    /// known. `None` when unknown.
    #[serde(default)]
    pub codec: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CacheManifestEntry {
    pub track_id: String,
    pub file_size: u64,
    pub last_accessed: i64, // Unix timestamp
    pub play_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthChallengeResponse {
    pub expires_in: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthRedeemResponse {
    pub ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_ladder_ceilings() {
        assert_eq!(Quality::Lowest.ceiling_kbps(), 16);
        assert_eq!(Quality::Low.ceiling_kbps(), 48);
        assert_eq!(Quality::Medium.ceiling_kbps(), 128);
        assert_eq!(Quality::High.ceiling_kbps(), 256);
        assert_eq!(Quality::Veryhigh.ceiling_kbps(), 512);
        assert_eq!(Quality::Extreme.ceiling_kbps(), 1024);
        assert_eq!(Quality::Best.ceiling_kbps(), 0);
        assert_eq!(Quality::Lossless.ceiling_kbps(), 0);
    }

    #[test]
    fn quality_unknown_label_is_skipped_not_rejected() {
        // Unknown label -> None (falls back to server default/best), not a 422.
        let p: TargetPayload = serde_json::from_str(r#"{"quality":"ultra"}"#).unwrap();
        assert_eq!(p.quality, None);
        // Valid labels parse case/whitespace-insensitively.
        let p: TargetPayload = serde_json::from_str(r#"{"quality":" LOW "}"#).unwrap();
        assert_eq!(p.quality, Some(Quality::Low));
        let p: TargetPayload = serde_json::from_str(r#"{"quality":"veryHigh"}"#).unwrap();
        assert_eq!(p.quality, Some(Quality::Veryhigh));
        // Absent field -> None (default).
        let p: TargetPayload = serde_json::from_str(r#"{"target_kbps":128}"#).unwrap();
        assert_eq!(p.quality, None);
    }

    #[test]
    fn resolve_ceiling_precedence() {
        // Explicit target_kbps > 0 wins over everything.
        assert_eq!(resolve_ceiling(Some(300), Some(Quality::Low), 1024), 300);
        // Some(0) / None target -> fall through to the quality label.
        assert_eq!(resolve_ceiling(Some(0), Some(Quality::Low), 1024), 48);
        assert_eq!(resolve_ceiling(None, Some(Quality::High), 1024), 256);
        // No label -> server default.
        assert_eq!(resolve_ceiling(None, None, 1024), 1024);
        // Absent label / default -> best (0).
        assert_eq!(resolve_ceiling(None, None, 0), 0);
        // best/lossless labels map to 0 (no ceiling).
        assert_eq!(resolve_ceiling(None, Some(Quality::Best), 1024), 0);
        assert_eq!(resolve_ceiling(None, Some(Quality::Lossless), 1024), 0);
    }
}
