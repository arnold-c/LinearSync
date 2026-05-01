use crate::error::AppError;
use crate::notes::frontmatter::{
    extract_ignored_properties, extract_linear_id_from_frontmatter, parse_frontmatter_map,
};
use crate::notes::paths::status_slug;
use crate::notes::sections::split_frontmatter;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct LocalNote {
    pub(crate) path: PathBuf,
    pub(crate) identifier: String,
    pub(crate) content: String,
    pub(crate) frontmatter: serde_yaml::Mapping,
    pub(crate) ignored_properties: Vec<String>,
    pub(crate) fallback_linear_id: Option<String>,
}

pub(crate) fn discover_markdown_notes(root: &Path, include_done: bool) -> Vec<PathBuf> {
    let mut notes = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(path) = stack.pop() {
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                if !include_done && is_done_directory(&entry_path) {
                    continue;
                }
                stack.push(entry_path);
            } else if entry_path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
            {
                notes.push(entry_path);
            }
        }
    }

    notes.sort();
    notes
}

pub(crate) fn discover_markdown_notes_for_issue(
    root: &Path,
    identifier: &str,
    include_done: bool,
) -> Vec<PathBuf> {
    discover_markdown_notes(root, include_done)
        .into_iter()
        .filter(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.trim() == identifier)
                .unwrap_or(false)
        })
        .collect()
}

pub(crate) fn parse_local_note(path: &Path) -> Result<LocalNote, AppError> {
    let content = fs::read_to_string(path).map_err(|error| {
        AppError::message(format!("failed to read {}: {}", path.display(), error))
    })?;
    let (frontmatter, _) = split_frontmatter(&content)
        .ok_or_else(|| AppError::message("note is missing YAML frontmatter"))?;
    let frontmatter = parse_frontmatter_map(frontmatter)
        .ok_or_else(|| AppError::message("note frontmatter is not valid YAML"))?;

    let identifier = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| AppError::message("note file name does not contain an issue identifier"))?
        .to_string();

    let ignored_properties = extract_ignored_properties(&content);
    let fallback_linear_id = extract_linear_id_from_frontmatter(&frontmatter);

    Ok(LocalNote {
        path: path.to_path_buf(),
        identifier,
        content,
        frontmatter,
        ignored_properties,
        fallback_linear_id,
    })
}

pub(crate) fn is_done_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("done"))
        .unwrap_or(false)
}

pub(crate) fn include_done_issue(
    include_done: bool,
    status: &str,
    desired_file_path: &Path,
    existing_file_path: &Path,
) -> bool {
    if status_slug(status) != "done" {
        return true;
    }

    include_done || existing_file_path != desired_file_path
}

pub(crate) fn find_issue_note_in_other_status(
    output_dir: &Path,
    identifier: &str,
) -> Option<PathBuf> {
    let target_name = format!("{}.md", identifier);
    let entries = fs::read_dir(output_dir).ok()?;

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if !entry_path.is_dir() {
            continue;
        }

        let candidate = entry_path.join(&target_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}
