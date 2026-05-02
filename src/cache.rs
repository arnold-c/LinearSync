use crate::error::AppError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
pub(crate) struct NoteIndexEntry {
    pub(crate) path: String,
    pub(crate) team_slug: String,
    pub(crate) status_slug: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SyncCache {
    #[serde(default = "cache_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) teams: BTreeMap<String, TeamCacheEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_local_index_at: Option<String>,
    #[serde(default)]
    pub(crate) notes: BTreeMap<String, NoteIndexEntry>,
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
            last_local_index_at: None,
            notes: BTreeMap::new(),
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
        self.indexed_note_entry(identifier).and_then(|entry| {
            indexed_note_path_from_entry(root, identifier, &entry.path, include_done)
        })
    }

    pub(crate) fn indexed_note_paths(&self, root: &Path, include_done: bool) -> Vec<PathBuf> {
        let mut seen = BTreeSet::new();
        let mut paths = Vec::new();

        for (identifier, relative_path) in self.indexed_note_path_entries() {
            if let Some(path) =
                indexed_note_path_from_entry(root, &identifier, &relative_path, include_done)
                && seen.insert(path.clone())
            {
                paths.push(path);
            }
        }

        paths.sort();
        paths
    }

    pub(crate) fn indexed_note_index_is_fresh(&self, root: &Path) -> bool {
        let Some(last_index_at) = self.last_local_index_at.as_deref() else {
            return false;
        };
        let Ok(last_index_at) = DateTime::parse_from_rfc3339(last_index_at) else {
            return false;
        };
        let last_index_at = last_index_at.with_timezone(&Utc);

        indexed_directories(root, self)
            .into_iter()
            .all(|directory| {
                directory_modified_at(&directory)
                    .is_some_and(|modified_at| modified_at <= last_index_at)
            })
    }

    pub(crate) fn mark_local_indexed_now(&mut self) {
        self.last_local_index_at = Some(Utc::now().to_rfc3339());
    }

    pub(crate) fn rebuild_note_index(&mut self, root: &Path, note_paths: &[PathBuf]) {
        let mut notes = BTreeMap::new();
        for note_path in note_paths {
            if let Some((identifier, entry)) = build_note_index_entry(root, note_path) {
                notes.insert(identifier, entry);
            }
        }
        self.notes = notes;
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
        self.update_indexed_note(root, identifier, note_path, team_slug, status_slug);

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

    fn indexed_note_path_entries(&self) -> Vec<(String, String)> {
        let mut entries = self
            .notes
            .iter()
            .map(|(identifier, entry)| (identifier.clone(), entry.path.clone()))
            .collect::<Vec<_>>();

        for (identifier, issue) in &self.issues {
            if self.notes.contains_key(identifier) {
                continue;
            }

            entries.push((identifier.clone(), issue.path.clone()));
        }

        entries
    }

    fn indexed_note_entry(&self, identifier: &str) -> Option<NoteIndexEntry> {
        self.notes.get(identifier).cloned().or_else(|| {
            self.issues.get(identifier).map(|issue| NoteIndexEntry {
                path: issue.path.clone(),
                team_slug: issue.team_slug.clone(),
                status_slug: issue.status_slug.clone(),
            })
        })
    }

    fn update_indexed_note(
        &mut self,
        root: &Path,
        identifier: &str,
        note_path: &Path,
        team_slug: &str,
        status_slug: &str,
    ) {
        let relative_path = note_path
            .strip_prefix(root)
            .unwrap_or(note_path)
            .to_string_lossy()
            .replace('\\', "/");

        self.notes.insert(
            identifier.to_string(),
            NoteIndexEntry {
                path: relative_path,
                team_slug: team_slug.to_string(),
                status_slug: status_slug.to_string(),
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

fn indexed_note_path_from_entry(
    root: &Path,
    identifier: &str,
    relative_path: &str,
    include_done: bool,
) -> Option<PathBuf> {
    let path = root.join(relative_path);

    if !path.is_file() || !path_matches_identifier(&path, identifier) {
        return None;
    }

    if !include_done && path_is_in_done_directory(&path) {
        return None;
    }

    Some(path)
}

fn build_note_index_entry(root: &Path, note_path: &Path) -> Option<(String, NoteIndexEntry)> {
    let identifier = note_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())?
        .to_string();

    let relative_path = note_path
        .strip_prefix(root)
        .unwrap_or(note_path)
        .to_string_lossy()
        .replace('\\', "/");
    let relative_path_buf = PathBuf::from(&relative_path);
    let path_components = relative_path_buf
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();

    let (team_slug, status_slug) = match path_components.as_slice() {
        [status, _file] => (String::new(), status.clone()),
        [team, status, _file, ..] => (team.clone(), status.clone()),
        _ => (String::new(), String::new()),
    };

    Some((
        identifier,
        NoteIndexEntry {
            path: relative_path,
            team_slug,
            status_slug,
        },
    ))
}

fn indexed_directories(root: &Path, cache: &SyncCache) -> BTreeSet<PathBuf> {
    let mut directories = BTreeSet::from([root.to_path_buf()]);

    for (_identifier, relative_path) in cache.indexed_note_path_entries() {
        let path = root.join(relative_path);
        let mut current = path.parent();
        while let Some(directory) = current {
            if directory == root {
                break;
            }
            directories.insert(directory.to_path_buf());
            current = directory.parent();
        }
    }

    directories
}

fn directory_modified_at(path: &Path) -> Option<DateTime<Utc>> {
    let modified_at = fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified_at))
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
    fn indexed_note_paths_include_rebuilt_notes_without_sync_baselines() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("in-progress").join("ABC-1.md");
        let second = dir.path().join("done").join("ABC-2.md");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, "test").unwrap();
        fs::write(&second, "test").unwrap();

        let mut cache = SyncCache::default();
        cache.rebuild_note_index(dir.path(), &[first.clone(), second.clone()]);

        assert_eq!(
            cache.indexed_note_paths(dir.path(), false),
            vec![first.clone()]
        );
        assert_eq!(
            cache.indexed_note_paths(dir.path(), true),
            vec![second, first]
        );
    }

    #[test]
    fn indexed_note_index_freshness_uses_cached_directories() {
        let dir = tempfile::tempdir().unwrap();
        let note_path = dir.path().join("in-progress").join("ABC-1.md");
        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "test").unwrap();

        let mut cache = SyncCache::default();
        cache.rebuild_note_index(dir.path(), std::slice::from_ref(&note_path));
        cache.last_local_index_at = Some("1970-01-01T00:00:00Z".to_string());
        assert!(!cache.indexed_note_index_is_fresh(dir.path()));

        cache.last_local_index_at = Some("2999-01-01T00:00:00Z".to_string());
        assert!(cache.indexed_note_index_is_fresh(dir.path()));
    }

    #[test]
    fn indexed_note_index_freshness_detects_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        let note_path = dir.path().join("in-progress").join("ABC-1.md");
        fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        fs::write(&note_path, "test").unwrap();

        let mut cache = SyncCache::default();
        cache.rebuild_note_index(dir.path(), std::slice::from_ref(&note_path));
        cache.mark_local_indexed_now();
        fs::remove_file(&note_path).unwrap();

        assert!(!cache.indexed_note_index_is_fresh(dir.path()));
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
