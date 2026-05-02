use crate::cache::{SyncCache, SyncState, compare_sync_state};
use crate::cli::ForceSelection;
use crate::error::AppError;
use crate::linear::client::{
    fetch_project_by_name, fetch_remote_issue_by_id, fetch_remote_issue_for_note, resolve_label,
    resolve_state, update_linear_issue,
};
use crate::linear::models::{PriorityInfo, RemoteIssue, get_priority_number};
use crate::notes::discovery::{
    LocalNote, discover_markdown_notes, discover_markdown_notes_for_issue, parse_local_note,
};
use crate::notes::frontmatter::{
    local_push_hash, local_push_hash_from_content, normalize_project_name, yaml_string,
    yaml_string_list,
};
use crate::notes::paths::{
    final_note_path_after_push, slugify_team_name, status_slug, write_note_to_path,
};
use crate::notes::reconcile::{
    FrontmatterWarning, insert_or_remove_conflict_section, managed_section_warning,
    push_frontmatter_diff_warning,
};
use crate::notes::render::{load_template, render_remote_issue_note};
use crate::notes::sections::{
    PushSyncWarning, insert_or_remove_note_location_warning, insert_or_remove_push_sync_section,
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

struct PushNoteFailure {
    error: AppError,
    sync_warning: PushSyncWarning,
    show_identifier_in_header: bool,
}

impl PushNoteFailure {
    fn missing_remote_issue(local_note: &LocalNote) -> Self {
        Self {
            error: AppError::message(format!(
                "Could not find a Linear issue for `{}`.",
                local_note.identifier
            )),
            sync_warning: PushSyncWarning {
                frontmatter: None,
                managed: None,
                notes: vec![format!(
                    "Could not find a Linear issue for `{}`. The note name is not changed automatically; verify the file name stem or `linear_id` frontmatter.",
                    local_note.identifier
                )],
            },
            show_identifier_in_header: true,
        }
    }

    fn fetch_error(error: AppError) -> Self {
        Self {
            sync_warning: PushSyncWarning {
                frontmatter: None,
                managed: None,
                notes: vec![error.to_string()],
            },
            error,
            show_identifier_in_header: false,
        }
    }
}

fn fetch_remote_issue_for_push(
    client: &Client,
    api_key: &str,
    local_note: &LocalNote,
) -> Result<RemoteIssue, PushNoteFailure> {
    match fetch_remote_issue_for_note(client, api_key, local_note) {
        Ok(Some(issue)) => Ok(issue),
        Ok(None) => Err(PushNoteFailure::missing_remote_issue(local_note)),
        Err(error) => Err(PushNoteFailure::fetch_error(error)),
    }
}

fn handle_push_note_failure(
    local_note: &LocalNote,
    note_content: &str,
    dry_run: bool,
    failure: PushNoteFailure,
) -> PushStats {
    if failure.show_identifier_in_header {
        println!(
            "{red}✗ Push error:{reset} {} ({})",
            local_note.path.display(),
            local_note.identifier,
            red = ANSI_RED,
            reset = ANSI_RESET,
        );
    } else {
        println!(
            "{red}✗ Push error:{reset} {}\n  {error}",
            local_note.path.display(),
            error = failure.error,
            red = ANSI_RED,
            reset = ANSI_RESET,
        );
    }

    if !dry_run
        && let Err(error) =
            write_push_sync_warning(&local_note.path, note_content, &failure.sync_warning)
    {
        println!(
            "{red}✗ Push error:{reset} {error}",
            red = ANSI_RED,
            reset = ANSI_RESET,
        );
    }

    PushStats {
        warnings: 1,
        errors: 1,
        ..PushStats::default()
    }
}

fn write_push_sync_warning(
    note_path: &Path,
    note_content: &str,
    warning: &PushSyncWarning,
) -> Result<(), AppError> {
    let note_content = insert_or_remove_push_sync_section(note_content, Some(warning));
    fs::write(note_path, note_content).map_err(|error| {
        AppError::message(format!(
            "failed to write {}: {}",
            note_path.display(),
            error
        ))
    })
}

fn persist_pushed_note(
    local_note: &LocalNote,
    note_content: &str,
    sync_warning: Option<&PushSyncWarning>,
    final_status_for_path: &str,
) -> Result<(), AppError> {
    let note_content = insert_or_remove_push_sync_section(note_content, sync_warning);
    let note_content = insert_or_remove_conflict_section(&note_content, None);
    let final_path = final_note_path_after_push(&local_note.path, final_status_for_path);

    write_note_to_path(&local_note.path, &final_path, &note_content).map_err(|error| {
        AppError::message(format!(
            "failed to write {}: {}",
            final_path.display(),
            error
        ))
    })
}

fn refetch_remote_issue_after_update(
    client: &Client,
    api_key: &str,
    issue_id: &str,
) -> Result<Option<RemoteIssue>, AppError> {
    fetch_remote_issue_by_id(client, api_key, issue_id).map_err(|error| {
        AppError::message(format!(
            "failed to refetch Linear issue `{issue_id}` after update: {error}"
        ))
    })
}

fn cache_warning_note(sync_state: SyncState) -> Option<&'static str> {
    match sync_state {
        SyncState::RemoteChangedOnly => Some(
            "Linear changed since the last sync, but the local pushable metadata has not. Run `pull` before pushing.",
        ),
        SyncState::BothChanged => Some(
            "Both the local note and the Linear issue changed since the last sync. Reconcile manually before pushing.",
        ),
        SyncState::Unknown | SyncState::InSync | SyncState::LocalChangedOnly => None,
    }
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

    let mut cache = SyncCache::load(&input_dir)?;
    let mut stats = PushStats::default();
    for note_path in note_paths {
        stats.scanned += 1;
        let note_stats = push_note(
            client,
            api_key,
            priority_values,
            &input_dir,
            &mut cache,
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

    if !dry_run {
        cache.save(&input_dir)?;
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
    input_dir: &Path,
    cache: &mut SyncCache,
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

    let remote_issue = match fetch_remote_issue_for_push(client, api_key, &local_note) {
        Ok(issue) => issue,
        Err(failure) => {
            return handle_push_note_failure(&local_note, &local_note.content, dry_run, failure);
        }
    };

    let current_local_push_hash =
        local_push_hash(&local_note.frontmatter, &local_note.ignored_properties);
    let sync_state = compare_sync_state(
        cache.get(&local_note.identifier),
        &current_local_push_hash,
        &remote_issue.updated_at,
    );

    let mut note_content = local_note.content.clone();
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
    let mut push_failed = false;

    if let Some(note) = cache_warning_note(sync_state) {
        println!(
            "{yellow}⚠ Sync review required:{reset} {} ({})",
            local_note.identifier,
            local_note.path.display(),
            yellow = ANSI_YELLOW,
            reset = ANSI_RESET,
        );
        println!("  {note}");
        notes.push(note.to_string());
    }

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

    if !matches!(
        sync_state,
        SyncState::RemoteChangedOnly | SyncState::BothChanged
    ) {
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
                    match update_linear_issue(client, api_key, &remote_issue.id, update_plan.input)
                    {
                        Ok(()) => {
                            stats.updated += 1;
                            if update_plan.will_move_to_done {
                                stats.moved += 1;
                                final_status_for_path = "done".to_string();
                                note_content =
                                    insert_or_remove_note_location_warning(&note_content, None);
                            }
                            match refetch_remote_issue_after_update(
                                client,
                                api_key,
                                &remote_issue.id,
                            ) {
                                Ok(Some(refetched_issue)) => {
                                    remote_issue = refetched_issue;
                                    let refreshed_now =
                                        Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
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
                                    managed_warning = managed_section_warning(
                                        &local_note.content,
                                        &remote_note.content,
                                    );
                                }
                                Ok(None) => {}
                                Err(error) => notes.push(error.to_string()),
                            }
                        }
                        Err(error) => {
                            notes.push(error.to_string());
                            println!(
                                "{red}✗ Push error:{reset} {}\n  {error}",
                                local_note.path.display(),
                                red = ANSI_RED,
                                reset = ANSI_RESET,
                            );
                            stats.errors += 1;
                            push_failed = true;
                        }
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

    let mut persisted_note = dry_run;
    if !dry_run {
        match persist_pushed_note(
            &local_note,
            &note_content,
            sync_warning.as_ref(),
            &final_status_for_path,
        ) {
            Ok(()) => persisted_note = true,
            Err(error) => {
                println!(
                    "{red}✗ Push error:{reset} {error}",
                    red = ANSI_RED,
                    reset = ANSI_RESET,
                );
                stats.errors += 1;
                push_failed = true;
            }
        }
    }

    if !dry_run
        && persisted_note
        && !push_failed
        && !matches!(
            sync_state,
            SyncState::RemoteChangedOnly | SyncState::BothChanged
        )
        && sync_warning
            .as_ref()
            .and_then(|warning| warning.frontmatter.as_ref())
            .is_none()
    {
        let final_note_path = final_note_path_after_push(&local_note.path, &final_status_for_path);
        let persisted_content = insert_or_remove_conflict_section(
            &insert_or_remove_push_sync_section(&note_content, sync_warning.as_ref()),
            None,
        );
        if let Some(final_local_push_hash) = local_push_hash_from_content(&persisted_content) {
            cache.update_issue(
                input_dir,
                &local_note.identifier,
                &final_note_path,
                &slugify_team_name(&remote_issue.team.name),
                &status_slug(&final_status_for_path),
                Some(&remote_issue.id),
                &remote_issue.updated_at,
                &final_local_push_hash,
            );
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
                        Err(error) => notes.push(error.to_string()),
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
