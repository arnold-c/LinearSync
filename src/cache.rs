use crate::error::AppError;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const CACHE_VERSION: u32 = 1;
const CACHE_DIR_NAME: &str = ".linear-sync";
const CACHE_FILE_NAME: &str = "cache.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct IssueCacheEntry {
    pub(crate) path: String,
    pub(crate) team_slug: String,
    pub(crate) status_slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) linear_id: Option<String>,
    pub(crate) last_sync_at: String,
    pub(crate) last_synced_linear_updated_at: String,
    pub(crate) last_synced_local_push_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct TeamCacheEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_remote_scan_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SyncCache {
    #[serde(default = "cache_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) teams: BTreeMap<String, TeamCacheEntry>,
    #[serde(default)]
    pub(crate) issues: BTreeMap<String, IssueCacheEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SyncState {
    Unknown,
    InSync,
    LocalChangedOnly,
    RemoteChangedOnly,
    BothChanged,
}

fn cache_version() -> u32 {
    CACHE_VERSION
}

impl Default for SyncCache {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            teams: BTreeMap::new(),
            issues: BTreeMap::new(),
        }
    }
}

impl SyncCache {
    pub(crate) fn load(root: &Path) -> Result<Self, AppError> {
        let path = cache_file_path(root);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path).map_err(|error| {
            AppError::message(format!("failed to read cache {}: {error}", path.display()))
        })?;
        let cache = serde_json::from_str::<Self>(&content).map_err(|error| {
            AppError::message(format!("failed to parse cache {}: {error}", path.display()))
        })?;

        if cache.version != CACHE_VERSION {
            return Ok(Self::default());
        }

        Ok(cache)
    }

    pub(crate) fn save(&self, root: &Path) -> Result<(), AppError> {
        let path = cache_file_path(root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                AppError::message(format!(
                    "failed to create cache directory {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let content = serde_json::to_string_pretty(self)
            .map_err(|error| AppError::message(format!("failed to serialize cache: {error}")))?;
        fs::write(&path, content).map_err(|error| {
            AppError::message(format!("failed to write cache {}: {error}", path.display()))
        })
    }

    pub(crate) fn get(&self, identifier: &str) -> Option<&IssueCacheEntry> {
        self.issues.get(identifier)
    }

    pub(crate) fn indexed_note_path(
        &self,
        root: &Path,
        identifier: &str,
        include_done: bool,
    ) -> Option<PathBuf> {
        let entry = self.get(identifier)?;
        let path = root.join(&entry.path);

        if !path.is_file() || !path_matches_identifier(&path, identifier) {
            return None;
        }

        if !include_done && path_is_in_done_directory(&path) {
            return None;
        }

        Some(path)
    }

    pub(crate) fn last_remote_scan_at(&self, team_id: &str) -> Option<&str> {
        self.teams
            .get(team_id)
            .and_then(|entry| entry.last_remote_scan_at.as_deref())
    }

    pub(crate) fn update_last_remote_scan_at(&mut self, team_id: &str, scanned_at: &str) {
        self.teams
            .entry(team_id.to_string())
            .or_default()
            .last_remote_scan_at = Some(scanned_at.to_string());
    }

    pub(crate) fn update_issue(
        &mut self,
        root: &Path,
        identifier: &str,
        note_path: &Path,
        team_slug: &str,
        status_slug: &str,
        linear_id: Option<&str>,
        remote_updated_at: &str,
        local_push_hash: &str,
    ) {
        let relative_path = note_path
            .strip_prefix(root)
            .unwrap_or(note_path)
            .to_string_lossy()
            .replace('\\', "/");

        self.issues.insert(
            identifier.to_string(),
            IssueCacheEntry {
                path: relative_path,
                team_slug: team_slug.to_string(),
                status_slug: status_slug.to_string(),
                linear_id: linear_id.map(ToString::to_string),
                last_sync_at: Utc::now().to_rfc3339(),
                last_synced_linear_updated_at: remote_updated_at.to_string(),
                last_synced_local_push_hash: local_push_hash.to_string(),
            },
        );
    }
}

pub(crate) fn compare_sync_state(
    entry: Option<&IssueCacheEntry>,
    current_local_push_hash: &str,
    current_linear_updated_at: &str,
) -> SyncState {
    let Some(entry) = entry else {
        return SyncState::Unknown;
    };

    let local_changed = entry.last_synced_local_push_hash != current_local_push_hash;
    let remote_changed = entry.last_synced_linear_updated_at != current_linear_updated_at;

    match (local_changed, remote_changed) {
        (false, false) => SyncState::InSync,
        (true, false) => SyncState::LocalChangedOnly,
        (false, true) => SyncState::RemoteChangedOnly,
        (true, true) => SyncState::BothChanged,
    }
}

fn cache_file_path(root: &Path) -> PathBuf {
    root.join(CACHE_DIR_NAME).join(CACHE_FILE_NAME)
}

fn path_matches_identifier(path: &Path, identifier: &str) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .map(|stem| stem == identifier)
        .unwrap_or(false)
}

fn path_is_in_done_directory(path: &Path) -> bool {
    let mut parent = path.parent();

    while let Some(dir) = parent {
        if dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.eq_ignore_ascii_case("done"))
            .unwrap_or(false)
        {
            return true;
        }

        parent = dir.parent();
    }

    false
}

#[cfg(test)]
mod tests {
    use super::{SyncCache, SyncState, compare_sync_state};
    use std::fs;

    #[test]
    fn compare_sync_state_covers_all_change_combinations() {
        let mut cache = SyncCache::default();
        cache.update_issue(
            std::path::Path::new("."),
            "ABC-1",
            std::path::Path::new("in-progress/ABC-1.md"),
            "team",
            "in-progress",
            Some("linear-id"),
            "2026-04-29T11:58:12Z",
            "sha256:one",
        );
        let entry = cache.get("ABC-1");

        assert_eq!(
            compare_sync_state(entry, "sha256:one", "2026-04-29T11:58:12Z"),
            SyncState::InSync
        );
        assert_eq!(
            compare_sync_state(entry, "sha256:two", "2026-04-29T11:58:12Z"),
            SyncState::LocalChangedOnly
        );
        assert_eq!(
            compare_sync_state(entry, "sha256:one", "2026-04-30T11:58:12Z"),
            SyncState::RemoteChangedOnly
        );
        assert_eq!(
            compare_sync_state(entry, "sha256:two", "2026-04-30T11:58:12Z"),
            SyncState::BothChanged
        );
    }

    #[test]
    fn indexed_note_path_returns_cached_note_when_valid() {
        let dir = tempfile::tempdir().unwrap();
        let note_path = dir.path().join("in-progress").join("ABC-1.md");
        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "test").unwrap();

        let mut cache = SyncCache::default();
        cache.update_issue(
            dir.path(),
            "ABC-1",
            &note_path,
            "team",
            "in-progress",
            Some("linear-id"),
            "2026-04-29T11:58:12Z",
            "sha256:one",
        );

        assert_eq!(
            cache.indexed_note_path(dir.path(), "ABC-1", true),
            Some(note_path)
        );
    }

    #[test]
    fn indexed_note_path_ignores_done_notes_when_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let note_path = dir.path().join("done").join("ABC-1.md");
        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "test").unwrap();

        let mut cache = SyncCache::default();
        cache.update_issue(
            dir.path(),
            "ABC-1",
            &note_path,
            "team",
            "done",
            Some("linear-id"),
            "2026-04-29T11:58:12Z",
            "sha256:one",
        );

        assert_eq!(cache.indexed_note_path(dir.path(), "ABC-1", false), None);
        assert_eq!(
            cache.indexed_note_path(dir.path(), "ABC-1", true),
            Some(note_path)
        );
    }

    #[test]
    fn indexed_note_path_rejects_stale_identifier_mismatches() {
        let dir = tempfile::tempdir().unwrap();
        let note_path = dir.path().join("in-progress").join("XYZ-9.md");
        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "test").unwrap();

        let mut cache = SyncCache::default();
        cache.update_issue(
            dir.path(),
            "ABC-1",
            &note_path,
            "team",
            "in-progress",
            Some("linear-id"),
            "2026-04-29T11:58:12Z",
            "sha256:one",
        );

        assert_eq!(cache.indexed_note_path(dir.path(), "ABC-1", true), None);
    }

    #[test]
    fn team_scan_marker_round_trips() {
        let mut cache = SyncCache::default();
        assert_eq!(cache.last_remote_scan_at("team-1"), None);

        cache.update_last_remote_scan_at("team-1", "2026-05-01T12:00:00Z");

        assert_eq!(
            cache.last_remote_scan_at("team-1"),
            Some("2026-05-01T12:00:00Z")
        );
    }
}
