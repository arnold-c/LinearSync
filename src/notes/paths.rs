use crate::notes::sections::NoteLocationWarning;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_OUTPUT_ROOT: &str = "linear-issues";

pub(crate) fn status_slug(status: &str) -> String {
    status.trim().to_lowercase().replace(' ', "-")
}

pub(crate) fn note_location_warning(
    current_path: &Path,
    desired_path: &Path,
    status: &str,
    identifier: &str,
) -> Option<NoteLocationWarning> {
    if current_path == desired_path {
        None
    } else {
        Some(NoteLocationWarning {
            desired_path: desired_path.to_path_buf(),
            status: status.to_string(),
            identifier: identifier.to_string(),
        })
    }
}

pub(crate) fn final_note_path_after_push(current_path: &Path, status: &str) -> PathBuf {
    if status_slug(status) != "done" {
        return current_path.to_path_buf();
    }

    let Some(status_dir) = current_path.parent() else {
        return current_path.to_path_buf();
    };
    let Some(root_dir) = status_dir.parent() else {
        return current_path.to_path_buf();
    };
    let Some(file_name) = current_path.file_name() else {
        return current_path.to_path_buf();
    };

    root_dir.join("done").join(file_name)
}

pub(crate) fn write_note_to_path(
    original_path: &Path,
    final_path: &Path,
    content: &str,
) -> io::Result<()> {
    if original_path != final_path && final_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("target note already exists at {}", final_path.display()),
        ));
    }

    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(final_path, content)?;

    if original_path != final_path && original_path.exists() {
        fs::remove_file(original_path)?;
    }

    Ok(())
}

pub(crate) fn file_path_for_issue(output_dir: &Path, status: &str, identifier: &str) -> PathBuf {
    output_dir
        .join(status_slug(status))
        .join(format!("{}.md", identifier))
}

pub(crate) fn default_output_root() -> PathBuf {
    PathBuf::from(DEFAULT_OUTPUT_ROOT)
}

pub(crate) fn default_output_root_for_all_teams(merge_all_teams: bool) -> PathBuf {
    if merge_all_teams {
        default_output_root().join("all-teams")
    } else {
        default_output_root()
    }
}

pub(crate) fn default_output_dir_for_team(team_name: &str) -> PathBuf {
    default_output_root().join(slugify_team_name(team_name))
}

pub(crate) fn slugify_team_name(team_name: &str) -> String {
    let slug = team_name
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();

    let mut compacted = String::new();
    let mut last_was_dash = false;
    for ch in slug.chars() {
        if ch == '-' {
            if !last_was_dash {
                compacted.push(ch);
            }
            last_was_dash = true;
        } else {
            compacted.push(ch);
            last_was_dash = false;
        }
    }

    compacted.trim_matches('-').to_string()
}
