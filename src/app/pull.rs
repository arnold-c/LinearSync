use crate::cli::{PullSelection, prompt_for_pull_selection, resolve_pull_selection};
use crate::linear::client::{fetch_required_issue, fetch_teams, graphql_request};
use crate::linear::models::{PriorityInfo, RemoteIssue, TeamInfo, get_priority_label};
use crate::notes::discovery::{find_issue_note_in_other_status, include_done_issue};
use crate::notes::paths::{file_path_for_issue, note_location_warning, slugify_team_name};
use crate::notes::reconcile::{MergeResult, merge_with_existing_note};
use crate::notes::render::{TemplateContext, default_markdown_content, load_template, render_template};
use crate::notes::sections::insert_or_remove_note_location_warning;
use crate::output::diff::{ANSI_YELLOW, ANSI_RESET, format_delta_patch, print_colored_diff, print_delta_output};
use chrono::Utc;
use reqwest::blocking::Client;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process;

#[derive(Default)]
pub(crate) struct PullStats {
    pub(crate) imported: usize,
    pub(crate) warnings: usize,
    pub(crate) delta_output: String,
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
) {
    let teams = fetch_teams(client, api_key);

    if teams.is_empty() {
        eprintln!("❌ Error: No Linear teams were found for this account.");
        process::exit(1);
    }

    let selection = if confirm {
        prompt_for_pull_selection(&teams, team_id, output_dir, merge_all_teams)
    } else {
        resolve_pull_selection(&teams, team_id, output_dir, merge_all_teams)
    };

    let template = load_template(template_path.as_deref());
    let selected_issue = issue_id
        .as_deref()
        .map(|identifier| fetch_required_issue(client, api_key, identifier));

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
            );
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
                );
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
) -> PullStats {
    let query = r#"
    query GetTeamIssues($teamId: String!) {
      team(id: $teamId) {
        issues {
          nodes {
            id
            identifier
            title
            url
            description
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
        }
      }
    }
    "#;

    let response = graphql_request(client, api_key, query, json!({ "teamId": team.id }));

    let issues = match response["data"]["team"]["issues"]["nodes"].as_array() {
        Some(arr) => arr,
        None => {
            eprintln!(
                "❌ Error: Could not find issues for team '{}' ({}).",
                team.name, team.id
            );
            process::exit(1);
        }
    };

    if !dry_run && !output_dir.exists() {
        fs::create_dir_all(output_dir).expect("Failed to create output directory");
    }

    let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut stats = PullStats::default();

    for issue in issues {
        let identifier = issue["identifier"].as_str().unwrap_or("UNKNOWN");
        if let Some(selected_issue) = selected_issue
            && selected_issue.identifier != identifier
        {
            continue;
        }
        let title = issue["title"].as_str().unwrap_or("No Title");
        let status = issue["state"]["name"].as_str().unwrap_or("Todo");
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
            find_issue_note_in_other_status(output_dir, identifier)
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
            if let Some(parent) = existing_file_path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                eprintln!("⚠️ Failed to create directory {:?}: {}", parent, e);
                false
            } else if let Err(e) = fs::write(&existing_file_path, &markdown_content) {
                eprintln!("⚠️ Failed to write file {}.md: {}", identifier, e);
                false
            } else {
                true
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
        }
    }

    stats
}
