use std::path::PathBuf;

pub(crate) const MANAGED_SECTION_START: &str = "<!-- linear-sync:managed:start -->";
pub(crate) const MANAGED_SECTION_END: &str = "<!-- linear-sync:managed:end -->";
pub(crate) const CONFLICT_SECTION_START: &str =
    "<!-- linear-sync:frontmatter-conflict:start -->";
pub(crate) const CONFLICT_SECTION_END: &str = "<!-- linear-sync:frontmatter-conflict:end -->";
pub(crate) const PUSH_SYNC_SECTION_START: &str = "<!-- linear-sync:push-sync:start -->";
pub(crate) const PUSH_SYNC_SECTION_END: &str = "<!-- linear-sync:push-sync:end -->";
pub(crate) const NOTE_LOCATION_SECTION_START: &str = "<!-- linear-sync:note-location:start -->";
pub(crate) const NOTE_LOCATION_SECTION_END: &str = "<!-- linear-sync:note-location:end -->";

pub(crate) fn ensure_managed_section(content: &str) -> String {
    if content.contains(MANAGED_SECTION_START) && content.contains(MANAGED_SECTION_END) {
        return content.to_string();
    }

    if let Some((frontmatter, body)) = split_frontmatter(content) {
        let body = body.trim_start_matches('\n');
        return format!("{frontmatter}\n{MANAGED_SECTION_START}\n{body}\n{MANAGED_SECTION_END}\n");
    }

    format!("{MANAGED_SECTION_START}\n{content}\n{MANAGED_SECTION_END}\n")
}

pub(crate) fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let frontmatter_end = 4 + end + 5;
    Some((&content[..frontmatter_end], &content[frontmatter_end..]))
}

pub(crate) struct ManagedSectionWarning {
    pub(crate) diff: String,
}

pub(crate) struct PushSyncWarning {
    pub(crate) frontmatter: Option<crate::FrontmatterWarning>,
    pub(crate) managed: Option<ManagedSectionWarning>,
    pub(crate) notes: Vec<String>,
}

pub(crate) struct NoteLocationWarning {
    pub(crate) desired_path: PathBuf,
    pub(crate) status: String,
    pub(crate) identifier: String,
}

pub(crate) fn insert_or_remove_generated_section(
    content: &str,
    start_marker: &str,
    end_marker: &str,
    block: Option<&str>,
) -> String {
    let without_existing = remove_section(content, start_marker, end_marker);
    let Some(block) = block else {
        return without_existing;
    };

    if let Some((frontmatter, body)) = split_frontmatter(&without_existing) {
        format!(
            "{frontmatter}\n\n{block}\n{}",
            body.trim_start_matches('\n')
        )
    } else {
        format!("{block}\n{without_existing}")
    }
}

pub(crate) fn insert_or_remove_note_location_warning(
    content: &str,
    warning: Option<&NoteLocationWarning>,
) -> String {
    let block = warning.map(|warning| {
        format!(
            "{NOTE_LOCATION_SECTION_START}\n> [!warning] Move this note in Obsidian\n> Linear reports `{}` as status `{}`.\n> This file was updated in place to preserve backlinks.\n> Move it in Obsidian to `{}` so the folder matches the status.\n{NOTE_LOCATION_SECTION_END}\n",
            warning.identifier,
            warning.status,
            warning.desired_path.display(),
        )
    });

    insert_or_remove_generated_section(
        content,
        NOTE_LOCATION_SECTION_START,
        NOTE_LOCATION_SECTION_END,
        block.as_deref(),
    )
}

pub(crate) fn insert_or_remove_push_sync_section(
    content: &str,
    warning: Option<&PushSyncWarning>,
) -> String {
    let block = warning.map(|warning| {
        let mut body = vec![
            "> [!warning] Linear push requires review".to_string(),
            "> This note still differs from Linear.".to_string(),
        ];

        if let Some(frontmatter) = &warning.frontmatter {
            body.push(
                "> Frontmatter differences were not fully pushed. Reconcile manually or run `push --force`."
                    .to_string(),
            );
            body.push(">".to_string());
            body.push("> Frontmatter diff:".to_string());
            body.push("> ```diff".to_string());
            body.extend(frontmatter.diff.lines().map(|line| format!("> {line}")));
            body.push("> ```".to_string());
        }

        if let Some(managed) = &warning.managed {
            body.push(">".to_string());
            body.push("> Managed block diff (edit the issue in Linear instead):".to_string());
            body.push("> ```diff".to_string());
            body.extend(managed.diff.lines().map(|line| format!("> {line}")));
            body.push("> ```".to_string());
        }

        if !warning.notes.is_empty() {
            body.push(">".to_string());
            body.push("> Notes:".to_string());
            body.extend(warning.notes.iter().map(|note| format!("> - {note}")));
        }

        format!(
            "{PUSH_SYNC_SECTION_START}\n{}\n{PUSH_SYNC_SECTION_END}\n",
            body.join("\n")
        )
    });

    insert_or_remove_generated_section(
        content,
        PUSH_SYNC_SECTION_START,
        PUSH_SYNC_SECTION_END,
        block.as_deref(),
    )
}

pub(crate) fn extract_managed_section_body(content: &str) -> Option<&str> {
    let managed = extract_managed_section(content)?;
    let managed = managed.strip_prefix(MANAGED_SECTION_START)?;
    let managed = managed.strip_suffix(MANAGED_SECTION_END)?;
    Some(managed.trim())
}

pub(crate) fn extract_managed_section(content: &str) -> Option<&str> {
    extract_section(content, MANAGED_SECTION_START, MANAGED_SECTION_END)
}

pub(crate) fn extract_section<'a>(
    content: &'a str,
    start_marker: &str,
    end_marker: &str,
) -> Option<&'a str> {
    let start = content.find(start_marker)?;
    let after_start = start + start_marker.len();
    let end_relative = content[after_start..].find(end_marker)?;
    let end = after_start + end_relative + end_marker.len();
    Some(&content[start..end])
}

pub(crate) fn remove_section(content: &str, start_marker: &str, end_marker: &str) -> String {
    match extract_section(content, start_marker, end_marker) {
        Some(section) => {
            let prefix = content
                .split_once(section)
                .map(|(before, _)| before)
                .unwrap_or("");
            let suffix = content
                .rsplit_once(section)
                .map(|(_, after)| after)
                .unwrap_or("");
            format!("{prefix}{suffix}")
        }
        None => content.to_string(),
    }
}
