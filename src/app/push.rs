use crate::cli::ForceSelection;
use crate::error::AppError;
use crate::linear::client::{
    fetch_project_by_name, fetch_remote_issue_by_id, fetch_remote_issue_for_note,
    resolve_label, resolve_state, update_linear_issue,
};
use crate::linear::models::{PriorityInfo, RemoteIssue, get_priority_number};
use crate::notes::discovery::{
    LocalNote, discover_markdown_notes, discover_markdown_notes_for_issue, parse_local_note,
};
use crate::notes::frontmatter::{normalize_project_name, yaml_string, yaml_string_list};
use crate::notes::paths::{final_note_path_after_push, status_slug, write_note_to_path};
use crate::notes::reconcile::{
    FrontmatterWarning, insert_or_remove_conflict_section, managed_section_warning,
    push_frontmatter_diff_warning,
};
use crate::notes::render::{load_template, render_remote_issue_note};
use crate::notes::sections::{
    PushSyncWarning, insert_or_remove_note_location_warning,
    insert_or_remove_push_sync_section,
};
use crate::output::diff::{ANSI_BLUE, ANSI_RED, ANSI_RESET, ANSI_YELLOW, print_push_diff};
use chrono::Utc;
use reqwest::blocking::Client;
use serde_json::{Map as JsonMap, Value, json};
use serde_yaml::Value as YamlValue;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct PushStats {
    pub(crate) scanned: usize,
    pub(crate) updated: usize,
    pub(crate) warnings: usize,
    pub(crate) errors: usize,
    pub(crate) moved: usize,
}

pub(crate) struct IssueUpdatePlan {
    pub(crate) input: JsonMap<String, Value>,
    pub(crate) updated_keys: Vec<String>,
    pub(crate) notes: Vec<String>,
    pub(crate) will_move_to_done: bool,
}

pub(crate) fn push_command(
    client: &Client,
    api_key: &str,
    priority_values: &[PriorityInfo],
    input_dir: PathBuf,
    issue_id: Option<String>,
    template_path: Option<PathBuf>,
    force_selection: ForceSelection,
    include_done: bool,
    dry_run: bool,
    use_delta: bool,
) -> Result<(), AppError> {
    let template = load_template(template_path.as_deref())?;
    let note_paths = match issue_id.as_deref() {
        Some(identifier) => discover_markdown_notes_for_issue(&input_dir, identifier, include_done),
        None => discover_markdown_notes(&input_dir, include_done),
    };

    if note_paths.is_empty() {
        match issue_id {
            Some(identifier) => {
                println!(
                    "No markdown notes found for {} under {}.",
                    identifier,
                    input_dir.display()
                );
            }
            None => println!("No markdown notes found under {}.", input_dir.display()),
        }
        return Ok(());
    }

    let mut stats = PushStats::default();
    for note_path in note_paths {
        stats.scanned += 1;
        let note_stats = push_note(
            client,
            api_key,
            priority_values,
            &note_path,
            template.as_deref(),
            &force_selection,
            dry_run,
            use_delta,
        );
        stats.updated += note_stats.updated;
        stats.warnings += note_stats.warnings;
        stats.errors += note_stats.errors;
        stats.moved += note_stats.moved;
    }

    if dry_run {
        println!(
            "Dry run complete: scanned {} notes ({} updates planned, {} moves planned, {} warnings, {} errors).",
            stats.scanned, stats.updated, stats.moved, stats.warnings, stats.errors
        );
    } else {
        println!(
            "Push complete: scanned {} notes ({} updated, {} moved, {} warnings, {} errors).",
            stats.scanned, stats.updated, stats.moved, stats.warnings, stats.errors
        );
    }

    Ok(())
}

pub(crate) fn push_note(
    client: &Client,
    api_key: &str,
    priority_values: &[PriorityInfo],
    note_path: &Path,
    template: Option<&str>,
    force_selection: &ForceSelection,
    dry_run: bool,
    use_delta: bool,
) -> PushStats {
    let mut stats = PushStats::default();

    let local_note = match parse_local_note(note_path) {
        Ok(note) => note,
        Err(error) => {
            println!(
                "{red}✗ Push error:{reset} {}\n  {error}",
                note_path.display(),
                red = ANSI_RED,
                reset = ANSI_RESET,
            );
            stats.errors += 1;
            return stats;
        }
    };

    let mut note_content = local_note.content.clone();

    let remote_issue = match fetch_remote_issue_for_note(client, api_key, &local_note) {
        Ok(Some(issue)) => issue,
        Ok(None) => {
            let warning = PushSyncWarning {
                frontmatter: None,
                managed: None,
                notes: vec![format!(
                    "Could not find a Linear issue for `{}`. The note name is not changed automatically; verify the file name stem or `linear_id` frontmatter.",
                    local_note.identifier
                )],
            };
            println!(
                "{red}✗ Push error:{reset} {} ({})",
                local_note.path.display(),
                local_note.identifier,
                red = ANSI_RED,
                reset = ANSI_RESET,
            );
            if !dry_run {
                note_content = insert_or_remove_push_sync_section(&note_content, Some(&warning));
                if let Err(error) = fs::write(&local_note.path, note_content) {
                    println!(
                        "{red}✗ Push error:{reset} failed to write {}: {}",
                        local_note.path.display(),
                        error,
                        red = ANSI_RED,
                        reset = ANSI_RESET,
                    );
                }
            }
            stats.errors += 1;
            stats.warnings += 1;
            return stats;
        }
        Err(error) => {
            let warning = PushSyncWarning {
                frontmatter: None,
                managed: None,
                notes: vec![error.clone()],
            };
            println!(
                "{red}✗ Push error:{reset} {}\n  {error}",
                local_note.path.display(),
                red = ANSI_RED,
                reset = ANSI_RESET,
            );
            if !dry_run {
                note_content = insert_or_remove_push_sync_section(&note_content, Some(&warning));
                if let Err(write_error) = fs::write(&local_note.path, note_content) {
                    println!(
                        "{red}✗ Push error:{reset} failed to write {}: {}",
                        local_note.path.display(),
                        write_error,
                        red = ANSI_RED,
                        reset = ANSI_RESET,
                    );
                }
            }
            stats.errors += 1;
            stats.warnings += 1;
            return stats;
        }
    };

    let mut remote_issue = remote_issue;
    let mut final_status_for_path = remote_issue.status.clone();
    let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut remote_note = render_remote_issue_note(&remote_issue, template, &now, priority_values);
    let mut frontmatter_warning = push_frontmatter_diff_warning(
        &local_note.content,
        &remote_note.content,
        &local_note.ignored_properties,
    );
    let mut managed_warning = managed_section_warning(&local_note.content, &remote_note.content);
    let mut notes = Vec::new();

    if let Some(warning) = &frontmatter_warning {
        println!(
            "{yellow}⚠ Frontmatter differs from Linear:{reset} {} ({})",
            local_note.identifier,
            local_note.path.display(),
            yellow = ANSI_YELLOW,
            reset = ANSI_RESET,
        );
        print_push_diff(
            use_delta,
            &local_note.identifier,
            &format!("{} / frontmatter", remote_issue.team.name),
            &local_note.path,
            &warning.diff,
        );
    }

    if let Some(warning) = &managed_warning {
        println!(
            "{yellow}⚠ Managed block differs from Linear:{reset} {} ({})",
            local_note.identifier,
            local_note.path.display(),
            yellow = ANSI_YELLOW,
            reset = ANSI_RESET,
        );
        print_push_diff(
            use_delta,
            &local_note.identifier,
            &format!("{} / managed block", remote_issue.team.name),
            &local_note.path,
            &warning.diff,
        );
        println!("  Edit the issue in Linear instead of editing the managed block locally.");
    }

    let force_keys = resolve_force_keys(force_selection, frontmatter_warning.as_ref());
    if let Some(force_keys) = force_keys {
        let update_plan = build_issue_update_input(
            client,
            api_key,
            priority_values,
            &remote_issue,
            &local_note,
            &force_keys,
        );

        notes.extend(update_plan.notes.clone());

        if !update_plan.input.is_empty() {
            if dry_run {
                println!(
                    "{blue}ℹ Planned push:{reset} {} -> {}",
                    local_note.identifier,
                    update_plan.updated_keys.join(", "),
                    blue = ANSI_BLUE,
                    reset = ANSI_RESET,
                );
                stats.updated += 1;
                if update_plan.will_move_to_done {
                    stats.moved += 1;
                }
            } else {
                match update_linear_issue(client, api_key, &remote_issue.id, update_plan.input) {
                    Ok(()) => {
                        stats.updated += 1;
                        if update_plan.will_move_to_done {
                            stats.moved += 1;
                            final_status_for_path = "done".to_string();
                            note_content =
                                insert_or_remove_note_location_warning(&note_content, None);
                        }
                        if let Ok(Some(refetched_issue)) =
                            fetch_remote_issue_by_id(client, api_key, &remote_issue.id)
                        {
                            remote_issue = refetched_issue;
                            let refreshed_now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                            remote_note = render_remote_issue_note(
                                &remote_issue,
                                template,
                                &refreshed_now,
                                priority_values,
                            );
                            frontmatter_warning = push_frontmatter_diff_warning(
                                &local_note.content,
                                &remote_note.content,
                                &local_note.ignored_properties,
                            );
                            managed_warning =
                                managed_section_warning(&local_note.content, &remote_note.content);
                        }
                    }
                    Err(error) => {
                        notes.push(error.clone());
                        println!(
                            "{red}✗ Push error:{reset} {}\n  {error}",
                            local_note.path.display(),
                            red = ANSI_RED,
                            reset = ANSI_RESET,
                        );
                        stats.errors += 1;
                    }
                }
            }
        }
    }

    let sync_warning =
        if frontmatter_warning.is_some() || managed_warning.is_some() || !notes.is_empty() {
            Some(PushSyncWarning {
                frontmatter: frontmatter_warning,
                managed: managed_warning,
                notes,
            })
        } else {
            None
        };

    if !dry_run {
        note_content = insert_or_remove_push_sync_section(&note_content, sync_warning.as_ref());
        note_content = insert_or_remove_conflict_section(&note_content, None);
        let final_path = final_note_path_after_push(&local_note.path, &final_status_for_path);
        if let Err(error) = write_note_to_path(&local_note.path, &final_path, &note_content) {
            println!(
                "{red}✗ Push error:{reset} failed to write {}: {}",
                final_path.display(),
                error,
                red = ANSI_RED,
                reset = ANSI_RESET,
            );
            stats.errors += 1;
        }
    }

    if sync_warning.is_some() {
        stats.warnings += 1;
    }

    stats
}

pub(crate) fn build_issue_update_input(
    client: &Client,
    api_key: &str,
    priority_values: &[PriorityInfo],
    remote_issue: &RemoteIssue,
    local_note: &LocalNote,
    selected_keys: &BTreeSet<String>,
) -> IssueUpdatePlan {
    let mut input = JsonMap::new();
    let mut updated_keys = Vec::new();
    let mut notes = Vec::new();
    let mut will_move_to_done = false;

    for key in selected_keys {
        let key_value = YamlValue::String(key.clone());
        let local_value = local_note.frontmatter.get(&key_value);

        match key.as_str() {
            "title" => match local_value.and_then(yaml_string) {
                Some(title) => {
                    input.insert("title".to_string(), json!(title));
                    updated_keys.push(key.clone());
                }
                None => notes.push("`title` is not a scalar string in the local note.".to_string()),
            },
            "status" => match local_value.and_then(yaml_string) {
                Some(status) => match resolve_state(&remote_issue.states, &status) {
                    Some(state) => {
                        input.insert("stateId".to_string(), json!(state.id));
                        updated_keys.push(key.clone());
                        will_move_to_done = status_slug(&state.name) == "done";
                    }
                    None => notes.push(format!(
                        "`status` value `{status}` does not match any workflow state in team `{}`.",
                        remote_issue.team.name
                    )),
                },
                None => notes.push("`status` is not a scalar string in the local note.".to_string()),
            },
            "priority" => {
                let parsed_priority = match local_value {
                    Some(YamlValue::String(s)) => get_priority_number(priority_values, s),
                    Some(YamlValue::Number(n)) => n.as_i64(),
                    _ => None,
                };
                match parsed_priority {
                    Some(priority) => {
                        input.insert("priority".to_string(), json!(priority));
                        updated_keys.push(key.clone());
                    }
                    None => notes.push("`priority` must be a valid priority string or integer in the local note.".to_string()),
                }
            }
            "project" => match local_value.and_then(yaml_string) {
                Some(project_name) => match normalize_project_name(&project_name) {
                    Some(project_name) => match fetch_project_by_name(client, api_key, &project_name)
                    {
                        Ok(Some(project)) => {
                            input.insert("projectId".to_string(), json!(project.id));
                            updated_keys.push(key.clone());
                        }
                        Ok(None) => notes.push(format!(
                            "No Linear project named `{project_name}` was found."
                        )),
                        Err(error) => notes.push(error),
                    },
                    None => {
                        input.insert("projectId".to_string(), Value::Null);
                        updated_keys.push(key.clone());
                    }
                },
                None if matches!(local_value, Some(YamlValue::Null)) => {
                    input.insert("projectId".to_string(), Value::Null);
                    updated_keys.push(key.clone());
                }
                None => notes.push("`project` is not a scalar string in the local note.".to_string()),
            },
            "tags" => match local_value.and_then(yaml_string_list) {
                Some(tags) => {
                    let mut label_ids = Vec::new();
                    let mut missing_labels = Vec::new();
                    for tag in tags {
                        match resolve_label(&remote_issue.available_labels, &tag) {
                            Some(label) => label_ids.push(label.id.clone()),
                            None => missing_labels.push(tag),
                        }
                    }

                    if missing_labels.is_empty() {
                        input.insert("labelIds".to_string(), json!(label_ids));
                        updated_keys.push(key.clone());
                    } else {
                        notes.push(format!(
                            "These tags were not found in team `{}`: {}.",
                            remote_issue.team.name,
                            missing_labels.join(", ")
                        ));
                    }
                }
                None if matches!(local_value, Some(YamlValue::Sequence(_))) => {
                    input.insert("labelIds".to_string(), json!([]));
                    updated_keys.push(key.clone());
                }
                None => notes.push("`tags` must be a YAML sequence or comma-separated string.".to_string()),
            },
            "linear_id" => notes.push("`linear_id` is read-only and cannot be pushed to Linear.".to_string()),
            "github_links" => notes.push(
                "`github_links` attachments are not pushed automatically; edit the issue in Linear."
                    .to_string(),
            ),
            other => notes.push(format!(
                "`{other}` is not a supported Linear push property."
            )),
        }
    }

    IssueUpdatePlan {
        input,
        updated_keys,
        notes,
        will_move_to_done,
    }
}

pub(crate) fn resolve_force_keys(
    force_selection: &ForceSelection,
    warning: Option<&FrontmatterWarning>,
) -> Option<BTreeSet<String>> {
    match force_selection {
        ForceSelection::None => None,
        ForceSelection::All => Some(
            warning
                .map(|warning| warning.keys.iter().cloned().collect())
                .unwrap_or_else(BTreeSet::new),
        ),
        ForceSelection::Selected(keys) => Some(keys.clone()),
    }
}
