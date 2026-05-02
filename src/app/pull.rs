use crate::cache::{SyncCache, SyncState, compare_sync_state};
use crate::cli::{PullSelection, prompt_for_pull_selection, resolve_pull_selection};
use crate::error::AppError;
use crate::linear::client::{fetch_required_issue, fetch_teams, graphql_request};
use crate::linear::models::{PriorityInfo, RemoteIssue, TeamInfo, get_priority_label};
use crate::notes::discovery::{
    find_issue_note_in_other_status, include_done_issue, parse_local_note,
};
use crate::notes::frontmatter::local_push_hash_from_content;
use crate::notes::paths::{
    file_path_for_issue, note_location_warning, slugify_team_name, status_slug,
};
use crate::notes::reconcile::{MergeResult, merge_with_existing_note};
use crate::notes::render::{
    TemplateContext, default_markdown_content, load_template, render_template,
};
use crate::notes::sections::insert_or_remove_note_location_warning;
use crate::output::diff::{
    ANSI_RESET, ANSI_YELLOW, format_delta_patch, print_colored_diff, print_delta_output,
};
use chrono::{Duration, Utc};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

const REMOTE_SCAN_OVERLAP_MINUTES: i64 = 5;
const FULL_PULL_QUERY: &str = r#"
query GetTeamIssuesPage($teamId: ID!, $cursor: String) {
  issues(
    first: 100
    after: $cursor
    orderBy: updatedAt
    filter: { team: { id: { eq: $teamId } } }
  ) {
    nodes {
      id
      identifier
      title
      url
      description
      updatedAt
      state {
        name
      }
      priority
      labels {
        nodes {
          name
        }
      }
      project {
        name
      }
      attachments {
        nodes {
          title
          url
        }
      }
    }
    pageInfo {
      endCursor
      hasNextPage
    }
  }
}
"#;
const INCREMENTAL_PULL_QUERY: &str = r#"
query GetTeamIssuesUpdatedSince($teamId: ID!, $cursor: String, $since: DateTimeOrDuration!) {
  issues(
    first: 100
    after: $cursor
    orderBy: updatedAt
    filter: {
      team: { id: { eq: $teamId } }
      updatedAt: { gte: $since }
    }
  ) {
    nodes {
      id
      identifier
      title
      url
      description
      updatedAt
      state {
        name
      }
      priority
      labels {
        nodes {
          name
        }
      }
      project {
        name
      }
      attachments {
        nodes {
          title
          url
        }
      }
    }
    pageInfo {
      endCursor
      hasNextPage
    }
  }
}
"#;

#[derive(Default)]
pub(crate) struct PullStats {
    pub(crate) imported: usize,
    pub(crate) warnings: usize,
    pub(crate) delta_output: String,
}

fn persist_pulled_note(note_path: &Path, markdown_content: &str) -> Result<(), AppError> {
    if let Some(parent) = note_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::message(format!(
                "Failed to create directory {}: {}",
                parent.display(),
                error
            ))
        })?;
    }

    fs::write(note_path, markdown_content).map_err(|error| {
        AppError::message(format!(
            "Failed to write {}: {}",
            note_path.display(),
            error
        ))
    })
}

fn print_pull_sync_warning(
    stats: &mut PullStats,
    identifier: &str,
    note_path: &Path,
    message: &str,
) {
    stats.warnings += 1;
    println!(
        "{yellow}⚠ Pull skipped:{reset} {} ({}) -> {}",
        identifier,
        note_path.display(),
        message,
        yellow = ANSI_YELLOW,
        reset = ANSI_RESET,
    );
}

fn selected_issue_for_team<'a>(
    selected_issue: Option<&'a RemoteIssue>,
    team: &TeamInfo,
) -> Option<&'a RemoteIssue> {
    selected_issue.filter(|issue| issue.team.id == team.id)
}

fn selected_issue_value(issue: &RemoteIssue) -> Value {
    json!({
        "id": issue.id,
        "identifier": issue.identifier,
        "title": issue.title,
        "url": issue.url,
        "description": issue.description,
        "updatedAt": issue.updated_at,
        "state": {
            "name": issue.status,
        },
        "priority": issue.priority,
        "labels": {
            "nodes": issue.labels.iter().map(|label| json!({ "name": label.name })).collect::<Vec<_>>(),
        },
        "project": {
            "name": issue.project.as_ref().map(|project| project.name.clone()).unwrap_or_default(),
        },
        "attachments": {
            "nodes": issue.attachments.iter().map(|url| json!({ "url": url })).collect::<Vec<_>>(),
        },
    })
}

fn incremental_pull_since(cache: &SyncCache, team: &TeamInfo) -> Option<String> {
    let last_scan = cache.last_remote_scan_at(&team.id)?;
    let parsed = chrono::DateTime::parse_from_rfc3339(last_scan).ok()?;
    Some(
        parsed
            .with_timezone(&Utc)
            .checked_sub_signed(Duration::minutes(REMOTE_SCAN_OVERLAP_MINUTES))?
            .to_rfc3339(),
    )
}

fn fetch_team_pull_issues(
    client: &Client,
    api_key: &str,
    team: &TeamInfo,
    cache: &SyncCache,
    selected_issue: Option<&RemoteIssue>,
) -> Result<(Vec<Value>, bool), AppError> {
    if let Some(issue) = selected_issue_for_team(selected_issue, team) {
        return Ok((vec![selected_issue_value(issue)], false));
    }

    let since = incremental_pull_since(cache, team);
    let query = if since.is_some() {
        INCREMENTAL_PULL_QUERY
    } else {
        FULL_PULL_QUERY
    };
    let mut cursor = None::<String>;
    let mut issues = Vec::new();

    loop {
        let variables = match &since {
            Some(since) => {
                json!({ "teamId": team.id, "cursor": cursor, "since": since })
            }
            None => json!({ "teamId": team.id, "cursor": cursor }),
        };
        let response = graphql_request(client, api_key, query, variables)?;

        let nodes = response["data"]["issues"]["nodes"]
            .as_array()
            .ok_or_else(|| {
                AppError::message(format!(
                    "Could not find issues for team '{}' ({}).",
                    team.name, team.id
                ))
            })?;
        issues.extend(nodes.iter().cloned());

        let page_info = &response["data"]["issues"]["pageInfo"];
        let has_next_page = page_info["hasNextPage"].as_bool().unwrap_or(false);
        if !has_next_page {
            break;
        }

        cursor = page_info["endCursor"].as_str().map(ToString::to_string);
        if cursor.is_none() {
            break;
        }
    }

    Ok((issues, true))
}

pub(crate) fn pull_command(
    client: &Client,
    api_key: &str,
    priority_values: &[PriorityInfo],
    team_id: Option<String>,
    output_dir: Option<PathBuf>,
    issue_id: Option<String>,
    template_path: Option<PathBuf>,
    merge_all_teams: bool,
    confirm: bool,
    force: bool,
    include_done: bool,
    dry_run: bool,
    use_delta: bool,
) -> Result<(), AppError> {
    let teams = fetch_teams(client, api_key)?;

    if teams.is_empty() {
        return Err(AppError::message(
            "No Linear teams were found for this account.",
        ));
    }

    let selection = if confirm {
        prompt_for_pull_selection(&teams, team_id, output_dir, merge_all_teams)?
    } else {
        resolve_pull_selection(&teams, team_id, output_dir, merge_all_teams)
    };

    let template = load_template(template_path.as_deref())?;
    let selected_issue = match issue_id.as_deref() {
        Some(identifier) => Some(fetch_required_issue(client, api_key, identifier)?),
        None => None,
    };

    match selection {
        PullSelection::SingleTeam { team, output_dir } => {
            let stats = pull_issues(
                client,
                api_key,
                priority_values,
                &team,
                &output_dir,
                selected_issue.as_ref(),
                template.as_deref(),
                force,
                include_done,
                dry_run,
                use_delta,
            )?;
            if use_delta && !stats.delta_output.is_empty() {
                print_delta_output(&stats.delta_output);
            }
            if dry_run {
                println!(
                    "Dry run complete: would import {} notes ({} warnings).",
                    stats.imported, stats.warnings
                );
            } else {
                println!(
                    "Imported {} notes ({} warnings).",
                    stats.imported, stats.warnings
                );
            }
        }
        PullSelection::AllTeams {
            root_output_dir,
            merge_all_teams,
        } => {
            let mut total = PullStats::default();
            for team in teams {
                let team_output_dir = if merge_all_teams {
                    root_output_dir.clone()
                } else {
                    root_output_dir.join(slugify_team_name(&team.name))
                };
                let stats = pull_issues(
                    client,
                    api_key,
                    priority_values,
                    &team,
                    &team_output_dir,
                    selected_issue.as_ref(),
                    template.as_deref(),
                    force,
                    include_done,
                    dry_run,
                    use_delta,
                )?;
                total.imported += stats.imported;
                total.warnings += stats.warnings;
                total.delta_output.push_str(&stats.delta_output);
            }
            if use_delta && !total.delta_output.is_empty() {
                print_delta_output(&total.delta_output);
            }
            if dry_run {
                println!(
                    "Dry run complete: would import {} notes ({} warnings).",
                    total.imported, total.warnings
                );
            } else {
                println!(
                    "Imported {} notes ({} warnings).",
                    total.imported, total.warnings
                );
            }
        }
    }

    Ok(())
}

pub(crate) fn pull_issues(
    client: &Client,
    api_key: &str,
    priority_values: &[PriorityInfo],
    team: &TeamInfo,
    output_dir: &PathBuf,
    selected_issue: Option<&RemoteIssue>,
    template: Option<&str>,
    force: bool,
    include_done: bool,
    dry_run: bool,
    use_delta: bool,
) -> Result<PullStats, AppError> {
    if !dry_run && !output_dir.exists() {
        fs::create_dir_all(output_dir)?;
    }

    let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut stats = PullStats::default();
    let mut cache = SyncCache::load(output_dir)?;
    let team_slug = slugify_team_name(&team.name);
    let (issues, update_scan_marker) =
        fetch_team_pull_issues(client, api_key, team, &cache, selected_issue)?;
    let remote_scan_completed_at = Utc::now().to_rfc3339();

    for issue in &issues {
        let identifier = issue["identifier"].as_str().unwrap_or("UNKNOWN");
        if let Some(selected_issue) = selected_issue
            && selected_issue.identifier != identifier
        {
            continue;
        }
        let title = issue["title"].as_str().unwrap_or("No Title");
        let status = issue["state"]["name"].as_str().unwrap_or("Todo");
        let updated_at = issue["updatedAt"].as_str().unwrap_or("");
        let priority_num = issue["priority"].as_i64().unwrap_or(0);
        let priority = get_priority_label(priority_values, priority_num);
        let url = issue["url"].as_str().unwrap_or("");
        let issue_id = issue["id"].as_str().unwrap_or("");
        let project = issue["project"]["name"].as_str().unwrap_or("");
        let description_section = issue["description"]
            .as_str()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(|description| {
                let formatted_description = description.replace("\n", "\n> ");
                format!(">[!info]+ Description\n> {formatted_description}\n\n")
            })
            .unwrap_or_default();

        let mut labels_yaml = String::new();
        if let Some(labels_nodes) = issue["labels"]["nodes"].as_array() {
            if !labels_nodes.is_empty() {
                labels_yaml.push_str("tags:\n");
                for label in labels_nodes {
                    if let Some(name) = label["name"].as_str() {
                        let clean_name = name.replace(' ', "-");
                        labels_yaml.push_str(&format!("  - {}\n", clean_name));
                    }
                }
            }
        }

        let mut gh_yaml = String::new();
        if let Some(attachments_nodes) = issue["attachments"]["nodes"].as_array() {
            let mut temp_gh = String::from("github_links:\n");
            let mut has_links = false;

            for attachment in attachments_nodes {
                if let Some(attachment_url) = attachment["url"].as_str() {
                    if attachment_url.contains("github.com")
                        && (attachment_url.contains("/pull/")
                            || attachment_url.contains("/issues/"))
                    {
                        temp_gh.push_str(&format!("  - \"{}\"\n", attachment_url));
                        has_links = true;
                    }
                }
            }

            if has_links {
                gh_yaml = temp_gh;
            }
        }

        let rendered_note = match template {
            Some(template) => render_template(
                template,
                &TemplateContext {
                    title,
                    status,
                    priority,
                    issue_id,
                    identifier,
                    url,
                    project,
                    description_section: &description_section,
                    labels_yaml: &labels_yaml,
                    gh_yaml: &gh_yaml,
                    now: &now,
                    team_name: &team.name,
                },
            ),
            None => default_markdown_content(
                title,
                status,
                priority,
                issue_id,
                &labels_yaml,
                &gh_yaml,
                project,
                &description_section,
                url,
                &now,
            ),
        };

        let markdown_content = rendered_note.content;

        let desired_file_path = file_path_for_issue(output_dir, status, identifier);
        let existing_file_path = if desired_file_path.exists() {
            desired_file_path.clone()
        } else {
            cache
                .indexed_note_path(output_dir, identifier, true)
                .or_else(|| find_issue_note_in_other_status(output_dir, identifier))
                .unwrap_or_else(|| desired_file_path.clone())
        };
        let location_warning =
            note_location_warning(&existing_file_path, &desired_file_path, status, identifier);
        if !include_done_issue(
            include_done,
            status,
            &desired_file_path,
            existing_file_path.as_path(),
        ) {
            continue;
        }

        if existing_file_path.exists() && !force {
            if let Ok(local_note) = parse_local_note(&existing_file_path) {
                let current_local_push_hash = crate::notes::frontmatter::local_push_hash(
                    &local_note.frontmatter,
                    &local_note.ignored_properties,
                );
                match compare_sync_state(
                    cache.get(identifier),
                    &current_local_push_hash,
                    updated_at,
                ) {
                    SyncState::InSync if location_warning.is_none() => continue,
                    SyncState::LocalChangedOnly => {
                        print_pull_sync_warning(
                            &mut stats,
                            identifier,
                            &existing_file_path,
                            "Local pushable metadata changed since the last sync, but Linear has not changed. Run `push` to update Linear before pulling.",
                        );
                        continue;
                    }
                    SyncState::BothChanged => {
                        print_pull_sync_warning(
                            &mut stats,
                            identifier,
                            &existing_file_path,
                            "Both the local note and the Linear issue changed since the last sync. Reconcile manually, then run `push` or `pull --force` as appropriate.",
                        );
                        continue;
                    }
                    SyncState::Unknown | SyncState::InSync | SyncState::RemoteChangedOnly => {}
                }
            }
        }

        let merge_result = if force {
            MergeResult {
                content: markdown_content,
                warning: None,
            }
        } else {
            merge_with_existing_note(
                &existing_file_path,
                &markdown_content,
                &rendered_note.ignored_properties,
            )
        };
        let markdown_content = insert_or_remove_note_location_warning(
            &merge_result.content,
            location_warning.as_ref(),
        );

        let wrote_note = if dry_run {
            true
        } else {
            match persist_pulled_note(&existing_file_path, &markdown_content) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("⚠️ {error}");
                    false
                }
            }
        };

        if wrote_note {
            stats.imported += 1;
            if let Some(warning) = location_warning {
                stats.warnings += 1;
                println!(
                    "{yellow}⚠ Note location mismatch:{reset} {} ({}) -> move in Obsidian to {}",
                    identifier,
                    existing_file_path.display(),
                    warning.desired_path.display(),
                    yellow = ANSI_YELLOW,
                    reset = ANSI_RESET,
                );
            }
            if let Some(warning) = merge_result.warning {
                stats.warnings += 1;
                println!(
                    "{yellow}⚠ Frontmatter conflict:{reset} {} ({}) -> {}",
                    identifier,
                    team.name,
                    existing_file_path.display(),
                    yellow = ANSI_YELLOW,
                    reset = ANSI_RESET,
                );
                if use_delta {
                    stats.delta_output.push_str(&format_delta_patch(
                        identifier,
                        &team.name,
                        &existing_file_path,
                        &warning.diff,
                    ));
                } else {
                    print_colored_diff(&warning.diff);
                }
            }
            if !dry_run
                && let Some(local_push_hash) = local_push_hash_from_content(&markdown_content)
            {
                cache.update_issue(
                    output_dir,
                    identifier,
                    &existing_file_path,
                    &team_slug,
                    &status_slug(status),
                    Some(issue_id),
                    updated_at,
                    &local_push_hash,
                );
            }
        }
    }

    if !dry_run {
        if update_scan_marker {
            cache.update_last_remote_scan_at(&team.id, &remote_scan_completed_at);
        }
        cache.save(output_dir)?;
    }

    Ok(stats)
}
