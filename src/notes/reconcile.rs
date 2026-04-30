use crate::notes::frontmatter::{
    collect_frontmatter_keys, parse_frontmatter_map, render_modified_yaml_value_diff,
    render_yaml_value_diff,
};
use crate::notes::sections::{
    CONFLICT_SECTION_END, CONFLICT_SECTION_START, NOTE_LOCATION_SECTION_END,
    NOTE_LOCATION_SECTION_START, PUSH_SYNC_SECTION_END, PUSH_SYNC_SECTION_START,
    ManagedSectionWarning, extract_managed_section, extract_managed_section_body,
    remove_section, split_frontmatter,
};
use serde_yaml::Value as YamlValue;
use std::fs;
use std::path::Path;

pub(crate) struct MergeResult {
    pub(crate) content: String,
    pub(crate) warning: Option<FrontmatterWarning>,
}

pub(crate) struct FrontmatterWarning {
    pub(crate) diff: String,
    pub(crate) keys: Vec<String>,
}

pub(crate) fn merge_with_existing_note(
    file_path: &Path,
    new_content: &str,
    ignored_properties: &[String],
) -> MergeResult {
    let existing = match fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(_) => {
            return MergeResult {
                content: new_content.to_string(),
                warning: None,
            };
        }
    };

    let user_content = extract_user_content(&existing);
    let warning = frontmatter_conflict_warning(&existing, new_content, ignored_properties);
    let content_with_frontmatter = merge_frontmatter(&existing, new_content);
    let content_with_conflict =
        insert_or_remove_conflict_section(&content_with_frontmatter, warning.as_ref());

    let content = if let Some(user_content) = user_content {
        let trimmed_new = content_with_conflict.trim_end();
        format!("{trimmed_new}\n\n{user_content}")
    } else {
        content_with_conflict
    };

    MergeResult { content, warning }
}

pub(crate) fn merge_frontmatter(existing: &str, new_content: &str) -> String {
    let Some((existing_frontmatter, _)) = split_frontmatter(existing) else {
        return new_content.to_string();
    };
    let Some((_, new_body)) = split_frontmatter(new_content) else {
        return new_content.to_string();
    };

    format!(
        "{existing_frontmatter}\n{}",
        new_body.trim_start_matches('\n')
    )
}

pub(crate) fn insert_or_remove_conflict_section(
    new_content: &str,
    warning: Option<&FrontmatterWarning>,
) -> String {
    let without_existing =
        remove_section(new_content, CONFLICT_SECTION_START, CONFLICT_SECTION_END);

    let Some(warning) = warning else {
        return without_existing;
    };

    let conflict_block = format!(
        "{CONFLICT_SECTION_START}\n> [!warning] Linear metadata conflict\n> The imported Linear metadata differs from this note's frontmatter.\n> Review the differences below and reconcile manually, or run `pull --force`.\n>\n> ```diff\n{}\n> ```\n{CONFLICT_SECTION_END}\n",
        warning
            .diff
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if let Some((frontmatter, body)) = split_frontmatter(&without_existing) {
        format!(
            "{frontmatter}\n\n{conflict_block}\n{}",
            body.trim_start_matches('\n')
        )
    } else {
        format!("{conflict_block}\n{without_existing}")
    }
}

pub(crate) fn frontmatter_conflict_warning(
    existing: &str,
    new_content: &str,
    ignored_properties: &[String],
) -> Option<FrontmatterWarning> {
    let (existing_frontmatter, _) = split_frontmatter(existing)?;
    let (new_frontmatter, _) = split_frontmatter(new_content)?;

    let existing_yaml = parse_frontmatter_map(existing_frontmatter)?;
    let new_yaml = parse_frontmatter_map(new_frontmatter)?;

    let managed_keys = [
        "title",
        "status",
        "linear_id",
        "tags",
        "github_links",
        "project",
    ];
    let mut keys = managed_keys
        .iter()
        .map(|key| key.to_string())
        .collect::<Vec<_>>();

    for key in existing_yaml.keys() {
        if let YamlValue::String(key) = key {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }

    keys.retain(|key| !ignored_properties.iter().any(|ignored| ignored == key));

    let mut diff_lines = Vec::new();

    for key in keys {
        let key_value = YamlValue::String(key.clone());
        let existing_value = existing_yaml.get(&key_value);
        let new_value = new_yaml.get(&key_value);

        if existing_value == new_value {
            continue;
        }

        match (existing_value, new_value) {
            (Some(old), Some(new)) => {
                diff_lines.extend(render_modified_yaml_value_diff(&key, old, new))
            }
            (Some(old), None) => diff_lines.extend(render_yaml_value_diff('-', &key, old)),
            (None, Some(new)) => diff_lines.extend(render_yaml_value_diff('+', &key, new)),
            (None, None) => {}
        }
    }

    if diff_lines.is_empty() {
        None
    } else {
        Some(FrontmatterWarning {
            diff: diff_lines.join("\n"),
            keys: Vec::new(),
        })
    }
}

pub(crate) fn extract_user_content(content: &str) -> Option<String> {
    let content = remove_section(content, CONFLICT_SECTION_START, CONFLICT_SECTION_END);
    let content = remove_section(&content, PUSH_SYNC_SECTION_START, PUSH_SYNC_SECTION_END);
    let content = remove_section(
        &content,
        NOTE_LOCATION_SECTION_START,
        NOTE_LOCATION_SECTION_END,
    );
    let managed = extract_managed_section(&content)?;
    let prefix = content
        .split_once(managed)
        .map(|(before, _)| before)
        .unwrap_or("");
    let suffix = content
        .rsplit_once(managed)
        .map(|(_, after)| after)
        .unwrap_or("");

    let prefix_without_frontmatter = if let Some((_, rest)) = split_frontmatter(prefix) {
        rest
    } else {
        prefix
    };

    let combined = format!("{}{}", prefix_without_frontmatter, suffix);
    let trimmed = combined.trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(format!("{}\n", trimmed))
    }
}

pub(crate) fn normalize_managed_section_for_diff(content: &str) -> Vec<String> {
    extract_managed_section_body(content)
        .unwrap_or("")
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim_start().starts_with("*Last synced:"))
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn render_text_diff(local: &[String], remote: &[String]) -> Option<String> {
    if local == remote {
        return None;
    }

    let max_len = local.len().max(remote.len());
    let mut diff = Vec::new();

    for index in 0..max_len {
        match (local.get(index), remote.get(index)) {
            (Some(left), Some(right)) if left == right => {}
            (Some(left), Some(right)) => {
                diff.push(format!("- {}", left));
                diff.push(format!("+ {}", right));
            }
            (Some(left), None) => diff.push(format!("- {}", left)),
            (None, Some(right)) => diff.push(format!("+ {}", right)),
            (None, None) => {}
        }
    }

    Some(diff.join("\n"))
}

pub(crate) fn managed_section_warning(
    local_content: &str,
    remote_content: &str,
) -> Option<ManagedSectionWarning> {
    let local_lines = normalize_managed_section_for_diff(local_content);
    let remote_lines = normalize_managed_section_for_diff(remote_content);
    render_text_diff(&local_lines, &remote_lines).map(|diff| ManagedSectionWarning { diff })
}

pub(crate) fn push_frontmatter_diff_warning(
    local_content: &str,
    remote_content: &str,
    ignored_properties: &[String],
) -> Option<FrontmatterWarning> {
    let (local_frontmatter, _) = split_frontmatter(local_content)?;
    let (remote_frontmatter, _) = split_frontmatter(remote_content)?;

    let local_yaml = parse_frontmatter_map(local_frontmatter)?;
    let remote_yaml = parse_frontmatter_map(remote_frontmatter)?;

    let keys = collect_frontmatter_keys(&local_yaml, &remote_yaml, ignored_properties);
    let mut diff_lines = Vec::new();
    let mut diff_keys = Vec::new();

    for key in keys {
        let key_value = YamlValue::String(key.clone());
        let local_value = local_yaml.get(&key_value);
        let remote_value = remote_yaml.get(&key_value);

        if local_value == remote_value {
            continue;
        }

        diff_keys.push(key.clone());
        match (local_value, remote_value) {
            (Some(old), Some(new)) => {
                diff_lines.extend(render_modified_yaml_value_diff(&key, old, new))
            }
            (Some(old), None) => diff_lines.extend(render_yaml_value_diff('-', &key, old)),
            (None, Some(new)) => diff_lines.extend(render_yaml_value_diff('+', &key, new)),
            (None, None) => {}
        }
    }

    if diff_lines.is_empty() {
        None
    } else {
        Some(FrontmatterWarning {
            diff: diff_lines.join("\n"),
            keys: diff_keys,
        })
    }
}
