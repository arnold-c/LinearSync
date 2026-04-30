mod cli;

use crate::cli::{Cli, Commands, ForceSelection, parse_force_selection};
use chrono::Utc;
use clap::Parser;
use dotenvy::dotenv;
use reqwest::blocking::Client;
use serde_json::{Map as JsonMap, Value, json};
use serde_yaml::Value as YamlValue;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::OnceLock;

const MANAGED_SECTION_START: &str = "<!-- linear-sync:managed:start -->";
const MANAGED_SECTION_END: &str = "<!-- linear-sync:managed:end -->";
const CONFLICT_SECTION_START: &str = "<!-- linear-sync:frontmatter-conflict:start -->";
const CONFLICT_SECTION_END: &str = "<!-- linear-sync:frontmatter-conflict:end -->";
const PUSH_SYNC_SECTION_START: &str = "<!-- linear-sync:push-sync:start -->";
const PUSH_SYNC_SECTION_END: &str = "<!-- linear-sync:push-sync:end -->";
const NOTE_LOCATION_SECTION_START: &str = "<!-- linear-sync:note-location:start -->";
const NOTE_LOCATION_SECTION_END: &str = "<!-- linear-sync:note-location:end -->";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_BLUE: &str = "\x1b[34m";
const ANSI_RESET: &str = "\x1b[0m";

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";
const DEFAULT_OUTPUT_ROOT: &str = "linear-issues";
const DEFAULT_TEMPLATE_PATH: &str = "template.md";
const ALL_TEAMS_OPTION: &str = "ALL TEAMS";

static INSTALLED_TEMPLATE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

#[derive(Clone, Debug)]
struct TeamInfo {
    id: String,
    name: String,
}

#[derive(Clone, Debug)]
struct PriorityInfo {
    priority: i64,
    label: String,
}

#[derive(Clone, Debug)]
struct WorkflowState {
    id: String,
    name: String,
}

#[derive(Clone, Debug)]
struct LabelInfo {
    id: String,
    name: String,
}

#[derive(Clone, Debug)]
struct ProjectInfo {
    id: String,
    name: String,
}

#[derive(Clone, Debug)]
struct RemoteIssue {
    id: String,
    identifier: String,
    title: String,
    url: String,
    description: String,
    status: String,
    priority: i64,
    team: TeamInfo,
    states: Vec<WorkflowState>,
    labels: Vec<LabelInfo>,
    available_labels: Vec<LabelInfo>,
    project: Option<ProjectInfo>,
    attachments: Vec<String>,
}

struct LocalNote {
    path: PathBuf,
    identifier: String,
    content: String,
    frontmatter: serde_yaml::Mapping,
    ignored_properties: Vec<String>,
    fallback_linear_id: Option<String>,
}

#[derive(Default)]
struct PushStats {
    scanned: usize,
    updated: usize,
    warnings: usize,
    errors: usize,
    moved: usize,
}

fn get_priority_label<'a>(priority_values: &'a [PriorityInfo], priority: i64) -> &'a str {
    priority_values
        .iter()
        .find(|v| v.priority == priority)
        .map(|v| v.label.as_str())
        .unwrap_or("No priority")
}

fn get_priority_number(priority_values: &[PriorityInfo], label: &str) -> Option<i64> {
    priority_values
        .iter()
        .find(|v| v.label.eq_ignore_ascii_case(label))
        .map(|v| v.priority)
        .or_else(|| label.parse::<i64>().ok())
}

pub fn run() {
    dotenv().ok();
    initialize_installed_template_path();

    let cli = Cli::parse();

    let api_key = match env::var("LINEAR_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("❌ Error: LINEAR_API_KEY is not set.");
            eprintln!("Please provide your Linear API key by doing one of the following:");
            eprintln!(
                "  1. Create a .env file in the directory where you run this command containing:"
            );
            eprintln!("     LINEAR_API_KEY=lin_api_your_key_here");
            eprintln!("  2. Export it directly in your shell:");
            eprintln!("     export LINEAR_API_KEY=lin_api_your_key_here\n");
            process::exit(1);
        }
    };

    let client = Client::new();

    match &cli.command {
        Commands::Pull {
            team_id,
            issue_id,
            output_dir,
            template,
            merge_all_teams,
            confirm,
            force,
            include_done,
            dry_run,
            no_delta,
        } => {
            let priority_values = fetch_priority_values(&client, &api_key);
            pull_command(
                &client,
                &api_key,
                &priority_values,
                team_id.clone(),
                output_dir.clone(),
                issue_id.clone(),
                template.clone(),
                *merge_all_teams,
                *confirm,
                *force,
                *include_done,
                *dry_run,
                !*no_delta,
            );
        }
        Commands::Push {
            input_dir,
            issue_id,
            template,
            force,
            include_done,
            dry_run,
            no_delta,
        } => {
            let priority_values = fetch_priority_values(&client, &api_key);
            push_command(
                &client,
                &api_key,
                &priority_values,
                input_dir.clone(),
                issue_id.clone(),
                template.clone(),
                parse_force_selection(force),
                *include_done,
                *dry_run,
                !*no_delta,
            );
        }
    }
}

fn pull_command(
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

fn push_command(
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
) {
    let template = load_template(template_path.as_deref());
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
        return;
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
}

fn push_note(
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

enum PullSelection {
    SingleTeam {
        team: TeamInfo,
        output_dir: PathBuf,
    },
    AllTeams {
        root_output_dir: PathBuf,
        merge_all_teams: bool,
    },
}

fn resolve_pull_selection(
    teams: &[TeamInfo],
    team_id: Option<String>,
    output_dir: Option<PathBuf>,
    merge_all_teams: bool,
) -> PullSelection {
    match team_id {
        Some(team_id) => {
            let team = teams
                .iter()
                .find(|team| team.id == team_id)
                .cloned()
                .unwrap_or_else(|| TeamInfo {
                    id: team_id,
                    name: String::from("team"),
                });

            let output_dir = output_dir.unwrap_or_else(|| default_output_dir_for_team(&team.name));

            PullSelection::SingleTeam { team, output_dir }
        }
        None => PullSelection::AllTeams {
            root_output_dir: output_dir
                .unwrap_or_else(|| default_output_root_for_all_teams(merge_all_teams)),
            merge_all_teams,
        },
    }
}

fn prompt_for_pull_selection(
    teams: &[TeamInfo],
    team_id: Option<String>,
    output_dir: Option<PathBuf>,
    merge_all_teams: bool,
) -> PullSelection {
    let preselected_index = team_id
        .as_ref()
        .and_then(|selected_id| teams.iter().position(|team| &team.id == selected_id))
        .map(|index| index + 1)
        .unwrap_or(0);

    println!("Select a team:");
    println!(
        "  0) {}{}",
        ALL_TEAMS_OPTION,
        if preselected_index == 0 {
            " [default]"
        } else {
            ""
        }
    );
    for (index, team) in teams.iter().enumerate() {
        let is_default = preselected_index == index + 1;
        println!(
            "  {}) {} ({}){}",
            index + 1,
            team.name,
            team.id,
            if is_default { " [default]" } else { "" }
        );
    }

    let selected_index = loop {
        print!("Enter selection number [{}]: ", preselected_index);
        io::stdout().flush().expect("failed to flush stdout");

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("failed to read selection");

        let trimmed = input.trim();
        if trimmed.is_empty() {
            break preselected_index;
        }

        match trimmed.parse::<usize>() {
            Ok(index) if index <= teams.len() => break index,
            _ => println!(
                "Invalid selection. Please enter a number from 0 to {}.",
                teams.len()
            ),
        }
    };

    if selected_index == 0 {
        let confirmed_merge_all_teams = prompt_for_merge_all_teams(merge_all_teams);
        let root_dir = output_dir
            .unwrap_or_else(|| default_output_root_for_all_teams(confirmed_merge_all_teams));
        let confirmed_root_dir = prompt_for_output_dir(
            &format!("Output directory for {}", ALL_TEAMS_OPTION),
            root_dir,
        );
        PullSelection::AllTeams {
            root_output_dir: confirmed_root_dir,
            merge_all_teams: confirmed_merge_all_teams,
        }
    } else {
        let team = teams[selected_index - 1].clone();
        let team_dir = output_dir.unwrap_or_else(|| default_output_dir_for_team(&team.name));
        let confirmed_team_dir =
            prompt_for_output_dir(&format!("Output directory for {}", team.name), team_dir);
        PullSelection::SingleTeam {
            team,
            output_dir: confirmed_team_dir,
        }
    }
}

fn prompt_for_merge_all_teams(default_value: bool) -> bool {
    let default_label = if default_value { "Y/n" } else { "y/N" };
    loop {
        print!(
            "Merge all teams into a single subdirectory? [{}]: ",
            default_label
        );
        io::stdout().flush().expect("failed to flush stdout");

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("failed to read merge confirmation");

        let trimmed = input.trim().to_lowercase();
        match trimmed.as_str() {
            "" => return default_value,
            "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => println!("Please enter y, yes, n, no, or press Enter to accept the default."),
        }
    }
}

fn prompt_for_output_dir(label: &str, default_dir: PathBuf) -> PathBuf {
    println!("{} [{}]", label, default_dir.display());
    print!("Press Enter to accept or type a different path: ");
    io::stdout().flush().expect("failed to flush stdout");

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("failed to read output directory");

    let trimmed = input.trim();
    if trimmed.is_empty() {
        default_dir
    } else {
        PathBuf::from(trimmed)
    }
}

fn fetch_teams(client: &Client, api_key: &str) -> Vec<TeamInfo> {
    let query = r#"
    query GetTeams {
      teams {
        nodes {
          id
          name
        }
      }
    }
    "#;

    let response = graphql_request(client, api_key, query, json!({}));
    let teams = response["data"]["teams"]["nodes"]
        .as_array()
        .unwrap_or_else(|| {
            eprintln!("❌ Error: Could not retrieve team list from Linear.");
            process::exit(1);
        });

    teams
        .iter()
        .filter_map(|team| {
            Some(TeamInfo {
                id: team["id"].as_str()?.to_string(),
                name: team["name"].as_str()?.to_string(),
            })
        })
        .collect()
}

fn fetch_priority_values(client: &Client, api_key: &str) -> Vec<PriorityInfo> {
    let query = r#"
    query GetPriorityValues {
      issuePriorityValues {
        priority
        label
      }
    }
    "#;

    let response = graphql_request(client, api_key, query, json!({}));
    let values = response["data"]["issuePriorityValues"]
        .as_array()
        .unwrap_or_else(|| {
            eprintln!("❌ Error: Could not retrieve issue priority values from Linear.");
            process::exit(1);
        });

    values
        .iter()
        .filter_map(|val| {
            Some(PriorityInfo {
                priority: val["priority"].as_i64()?,
                label: val["label"].as_str()?.to_string(),
            })
        })
        .collect()
}

fn fetch_remote_issue_for_note(
    client: &Client,
    api_key: &str,
    local_note: &LocalNote,
) -> Result<Option<RemoteIssue>, String> {
    if let Some(issue) = fetch_remote_issue_by_identifier(client, api_key, &local_note.identifier)?
    {
        return Ok(Some(issue));
    }

    match &local_note.fallback_linear_id {
        Some(linear_id) if linear_id != &local_note.identifier => {
            fetch_remote_issue_by_id(client, api_key, linear_id)
        }
        _ => Ok(None),
    }
}

fn fetch_remote_issue_by_identifier(
    client: &Client,
    api_key: &str,
    identifier: &str,
) -> Result<Option<RemoteIssue>, String> {
    let mut last_shape_error =
        match fetch_remote_issue_by_issue_v2_identifier(client, api_key, identifier) {
            Ok(Some(issue)) => return Ok(Some(issue)),
            Ok(None) => return Ok(None),
            Err(error) if is_graphql_shape_error(&error) => Some(error),
            Err(error) => return Err(error),
        };

    match fetch_remote_issue_by_id(client, api_key, identifier) {
        Ok(Some(issue)) => return Ok(Some(issue)),
        Ok(None) => {}
        Err(error) if is_graphql_shape_error(&error) => last_shape_error = Some(error),
        Err(error) => return Err(error),
    }

    if let Some((team_key, issue_number)) = parse_issue_identifier(identifier) {
        match fetch_remote_issue_by_team_and_number(client, api_key, &team_key, issue_number) {
            Ok(Some(issue)) => return Ok(Some(issue)),
            Ok(None) => {}
            Err(error) if is_graphql_shape_error(&error) => last_shape_error = Some(error),
            Err(error) => return Err(error),
        }
    }

    match last_shape_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

fn fetch_remote_issue_by_issue_v2_identifier(
    client: &Client,
    api_key: &str,
    identifier: &str,
) -> Result<Option<RemoteIssue>, String> {
    let query = r#"
    query GetIssueByIdentifier($identifier: String!) {
      issueV2(identifier: $identifier) {
        id
        identifier
        title
        url
        description
        priority
        state {
          id
          name
        }
        team {
          id
          name
          states {
            nodes {
              id
              name
            }
          }
          labels {
            nodes {
              id
              name
            }
          }
        }
        labels {
          nodes {
            id
            name
          }
        }
        project {
          id
          name
        }
        attachments {
          nodes {
            url
          }
        }
      }
    }
    "#;

    let response =
        graphql_request_result(client, api_key, query, json!({ "identifier": identifier }))?;
    let issue = response["data"]["issueV2"]
        .as_object()
        .cloned()
        .map(Value::Object);
    issue.map(parse_remote_issue).transpose()
}

fn fetch_remote_issue_by_team_and_number(
    client: &Client,
    api_key: &str,
    team_key: &str,
    issue_number: i64,
) -> Result<Option<RemoteIssue>, String> {
    let query = r#"
    query GetIssueByTeamAndNumber($teamKey: String!, $issueNumber: Int!) {
      team(id: $teamKey) {
        issues(filter: { number: { eq: $issueNumber } }) {
          nodes {
            id
            identifier
            title
            url
            description
            priority
            state {
              id
              name
            }
            team {
              id
              name
              states {
                nodes {
                  id
                  name
                }
              }
              labels {
                nodes {
                  id
                  name
                }
              }
            }
            labels {
              nodes {
                id
                name
              }
            }
            project {
              id
              name
            }
            attachments {
              nodes {
                url
              }
            }
          }
        }
      }
    }
    "#;

    let response = graphql_request_result(
        client,
        api_key,
        query,
        json!({ "teamKey": team_key, "issueNumber": issue_number }),
    )?;
    let issue = response["data"]["team"]["issues"]["nodes"]
        .as_array()
        .and_then(|issues| issues.first())
        .cloned();

    issue.map(parse_remote_issue).transpose()
}

fn parse_issue_identifier(identifier: &str) -> Option<(String, i64)> {
    let (team_key, issue_number) = identifier.split_once('-')?;
    let team_key = team_key.trim();
    let issue_number = issue_number.trim().parse::<i64>().ok()?;

    if team_key.is_empty() {
        None
    } else {
        Some((team_key.to_string(), issue_number))
    }
}

fn is_graphql_shape_error(error: &str) -> bool {
    error.contains("GRAPHQL_VALIDATION_FAILED")
        || error.contains("Field \"")
        || error.contains("Cannot query field")
        || error.contains("Unknown argument")
}

fn fetch_remote_issue_by_id(
    client: &Client,
    api_key: &str,
    issue_id: &str,
) -> Result<Option<RemoteIssue>, String> {
    let query = r#"
    query GetIssueById($id: String!) {
      issue(id: $id) {
        id
        identifier
        title
        url
        description
        priority
        state {
          id
          name
        }
        team {
          id
          name
          states {
            nodes {
              id
              name
            }
          }
          labels {
            nodes {
              id
              name
            }
          }
        }
        labels {
          nodes {
            id
            name
          }
        }
        project {
          id
          name
        }
        attachments {
          nodes {
            url
          }
        }
      }
    }
    "#;

    let response = graphql_request_result(client, api_key, query, json!({ "id": issue_id }))?;
    let issue = response["data"]["issue"]
        .as_object()
        .cloned()
        .map(Value::Object);
    issue.map(parse_remote_issue).transpose()
}

fn parse_remote_issue(issue: Value) -> Result<RemoteIssue, String> {
    let id = issue["id"]
        .as_str()
        .ok_or_else(|| "Linear issue is missing an id".to_string())?
        .to_string();
    let identifier = issue["identifier"]
        .as_str()
        .ok_or_else(|| "Linear issue is missing an identifier".to_string())?
        .to_string();
    let title = issue["title"].as_str().unwrap_or("No Title").to_string();
    let url = issue["url"].as_str().unwrap_or("").to_string();
    let description = issue["description"].as_str().unwrap_or("").to_string();
    let status = issue["state"]["name"]
        .as_str()
        .unwrap_or("Todo")
        .to_string();
    let priority = issue["priority"].as_i64().unwrap_or(0);

    let team = TeamInfo {
        id: issue["team"]["id"].as_str().unwrap_or("").to_string(),
        name: issue["team"]["name"].as_str().unwrap_or("team").to_string(),
    };

    let states = issue["team"]["states"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|state| {
            Some(WorkflowState {
                id: state["id"].as_str()?.to_string(),
                name: state["name"].as_str()?.to_string(),
            })
        })
        .collect::<Vec<_>>();

    let labels = issue["labels"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|label| {
            Some(LabelInfo {
                id: label["id"].as_str()?.to_string(),
                name: label["name"].as_str()?.to_string(),
            })
        })
        .collect::<Vec<_>>();

    let available_labels = issue["team"]["labels"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|label| {
            Some(LabelInfo {
                id: label["id"].as_str()?.to_string(),
                name: label["name"].as_str()?.to_string(),
            })
        })
        .collect::<Vec<_>>();

    let project = issue["project"]["id"]
        .as_str()
        .map(|project_id| ProjectInfo {
            id: project_id.to_string(),
            name: issue["project"]["name"].as_str().unwrap_or("").to_string(),
        });

    let attachments = issue["attachments"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|attachment| attachment["url"].as_str().map(ToString::to_string))
        .collect::<Vec<_>>();

    Ok(RemoteIssue {
        id,
        identifier,
        title,
        url,
        description,
        status,
        priority,
        team,
        states,
        labels,
        available_labels,
        project,
        attachments,
    })
}

fn graphql_request_result(
    client: &Client,
    api_key: &str,
    query: &str,
    variables: Value,
) -> Result<Value, String> {
    let response = client
        .post(LINEAR_API_URL)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .map_err(|error| format!("failed to send request to Linear API: {error}"))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("failed to read Linear API response: {error}"))?;

    if !status.is_success() {
        return Err(format!("Linear API request failed with {status}: {body}"));
    }

    let value = serde_json::from_str::<Value>(&body)
        .map_err(|error| format!("failed to parse Linear API response JSON: {error}"))?;

    if let Some(errors) = value["errors"].as_array()
        && !errors.is_empty()
    {
        return Err(format!(
            "Linear API returned GraphQL errors: {}",
            value["errors"]
        ));
    }

    Ok(value)
}

fn resolve_state<'a>(
    states: &'a [WorkflowState],
    desired_status: &str,
) -> Option<&'a WorkflowState> {
    states
        .iter()
        .find(|state| state.name == desired_status)
        .or_else(|| {
            states
                .iter()
                .find(|state| state.name.eq_ignore_ascii_case(desired_status))
        })
        .or_else(|| {
            let desired_slug = status_slug(desired_status);
            states
                .iter()
                .find(|state| status_slug(&state.name) == desired_slug)
        })
}

fn resolve_label<'a>(labels: &'a [LabelInfo], desired_label: &str) -> Option<&'a LabelInfo> {
    labels
        .iter()
        .find(|label| label.name == desired_label)
        .or_else(|| {
            labels
                .iter()
                .find(|label| label.name.eq_ignore_ascii_case(desired_label))
        })
        .or_else(|| {
            let desired_slug = status_slug(desired_label);
            labels
                .iter()
                .find(|label| status_slug(&label.name) == desired_slug)
        })
}

fn fetch_project_by_name(
    client: &Client,
    api_key: &str,
    project_name: &str,
) -> Result<Option<ProjectInfo>, String> {
    let query = r#"
    query GetProjectByName($projectName: String!) {
      projects(filter: { name: { eq: $projectName } }) {
        nodes {
          id
          name
        }
      }
    }
    "#;

    let response = graphql_request_result(
        client,
        api_key,
        query,
        json!({ "projectName": project_name }),
    )?;

    Ok(response["data"]["projects"]["nodes"]
        .as_array()
        .and_then(|projects| projects.first())
        .and_then(|project| {
            Some(ProjectInfo {
                id: project["id"].as_str()?.to_string(),
                name: project["name"].as_str()?.to_string(),
            })
        }))
}

fn build_issue_update_input(
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

fn update_linear_issue(
    client: &Client,
    api_key: &str,
    issue_id: &str,
    input: JsonMap<String, Value>,
) -> Result<(), String> {
    let query = r#"
    mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {
      issueUpdate(id: $id, input: $input) {
        success
      }
    }
    "#;

    let response = graphql_request_result(
        client,
        api_key,
        query,
        json!({ "id": issue_id, "input": Value::Object(input) }),
    )?;

    let success = response["data"]["issueUpdate"]["success"]
        .as_bool()
        .unwrap_or(false);
    if success {
        Ok(())
    } else {
        Err("Linear issue update did not report success.".to_string())
    }
}

#[derive(Default)]
struct PullStats {
    imported: usize,
    warnings: usize,
    delta_output: String,
}

fn pull_issues(
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

struct TemplateContext<'a> {
    title: &'a str,
    status: &'a str,
    priority: &'a str,
    issue_id: &'a str,
    identifier: &'a str,
    url: &'a str,
    project: &'a str,
    description_section: &'a str,
    labels_yaml: &'a str,
    gh_yaml: &'a str,
    now: &'a str,
    team_name: &'a str,
}

struct RenderedNote {
    content: String,
    ignored_properties: Vec<String>,
}

fn load_template(template_path: Option<&Path>) -> Option<String> {
    let path = template_path
        .map(Path::to_path_buf)
        .or_else(default_template_path_if_present);

    path.map(|path| {
        fs::read_to_string(&path).unwrap_or_else(|error| {
            eprintln!(
                "❌ Error: Failed to read template file '{}': {}",
                path.display(),
                error
            );
            process::exit(1);
        })
    })
}

fn default_template_path_if_present() -> Option<PathBuf> {
    default_template_search_paths()
        .into_iter()
        .find(|path| path.is_file())
}

fn default_template_search_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(DEFAULT_TEMPLATE_PATH)];

    if let Some(installed_path) = installed_template_path() {
        paths.push(installed_path.clone());
    }

    paths
}

fn initialize_installed_template_path() {
    let _ = INSTALLED_TEMPLATE_PATH.get_or_init(|| {
        env::current_exe().ok().map(|exe_path| {
            exe_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(DEFAULT_TEMPLATE_PATH)
        })
    });
}

fn installed_template_path() -> Option<&'static PathBuf> {
    INSTALLED_TEMPLATE_PATH.get().and_then(|path| path.as_ref())
}

fn render_template(template: &str, context: &TemplateContext<'_>) -> RenderedNote {
    let rendered = template
        .replace("{{title}}", context.title)
        .replace("{{status}}", context.status)
        .replace("{{priority}}", context.priority)
        .replace("{{linear_id}}", context.issue_id)
        .replace("{{identifier}}", context.identifier)
        .replace("{{url}}", context.url)
        .replace("{{project}}", context.project)
        .replace("{{description_section}}", context.description_section)
        .replace("{{labels_yaml}}", context.labels_yaml)
        .replace("{{github_links_yaml}}", context.gh_yaml)
        .replace("{{last_synced}}", context.now)
        .replace("{{team_name}}", context.team_name);

    let ignored_properties = extract_ignored_properties(&rendered);

    RenderedNote {
        content: ensure_managed_section(&rendered),
        ignored_properties,
    }
}

fn description_section_from_text(description: &str) -> String {
    let description = description.trim();
    if description.is_empty() {
        String::new()
    } else {
        let formatted_description = description.replace("\n", "\n> ");
        format!(">[!info]+ Description\n> {formatted_description}\n\n")
    }
}

fn labels_yaml_from_names(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }

    let mut labels_yaml = String::from("tags:\n");
    for name in names {
        labels_yaml.push_str(&format!("  - {}\n", name.replace(' ', "-")));
    }
    labels_yaml
}

fn github_links_yaml_from_urls(urls: &[String]) -> String {
    let github_urls = urls
        .iter()
        .filter(|url| {
            url.contains("github.com") && (url.contains("/pull/") || url.contains("/issues/"))
        })
        .collect::<Vec<_>>();

    if github_urls.is_empty() {
        return String::new();
    }

    let mut gh_yaml = String::from("github_links:\n");
    for url in github_urls {
        gh_yaml.push_str(&format!("  - \"{}\"\n", url));
    }
    gh_yaml
}

fn render_remote_issue_note(
    issue: &RemoteIssue,
    template: Option<&str>,
    now: &str,
    priority_values: &[PriorityInfo],
) -> RenderedNote {
    let description_section = description_section_from_text(&issue.description);
    let labels_yaml = labels_yaml_from_names(
        &issue
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect::<Vec<_>>(),
    );
    let gh_yaml = github_links_yaml_from_urls(&issue.attachments);
    let project_name = issue
        .project
        .as_ref()
        .map(|project| project.name.as_str())
        .unwrap_or("");

    let mut rendered = match template {
        Some(template) => render_template(
            template,
            &TemplateContext {
                title: &issue.title,
                status: &issue.status,
                priority: get_priority_label(priority_values, issue.priority),
                issue_id: &issue.id,
                identifier: &issue.identifier,
                url: &issue.url,
                project: project_name,
                description_section: &description_section,
                labels_yaml: &labels_yaml,
                gh_yaml: &gh_yaml,
                now,
                team_name: &issue.team.name,
            },
        ),
        None => default_markdown_content(
            &issue.title,
            &issue.status,
            get_priority_label(priority_values, issue.priority),
            &issue.id,
            &labels_yaml,
            &gh_yaml,
            project_name,
            &description_section,
            &issue.url,
            now,
        ),
    };

    rendered.content = override_frontmatter_value(
        &rendered.content,
        "priority",
        YamlValue::String(get_priority_label(priority_values, issue.priority).to_string()),
    );
    rendered
}

fn default_markdown_content(
    title: &str,
    status: &str,
    priority: &str,
    issue_id: &str,
    labels_yaml: &str,
    gh_yaml: &str,
    project: &str,
    description_section: &str,
    url: &str,
    now: &str,
) -> RenderedNote {
    let managed = format!(
        r#"---
title: "{title}"
status: "{status}"
priority: "{priority}"
linear_id: "[{issue_id}]({url})"
{labels_yaml}{gh_yaml}project: "[[{project}]]"
---

{MANAGED_SECTION_START}
{description_section}---
*Last synced: {now}*
{MANAGED_SECTION_END}

## My notes
"#
    );

    RenderedNote {
        content: managed,
        ignored_properties: Vec::new(),
    }
}

fn ensure_managed_section(content: &str) -> String {
    if content.contains(MANAGED_SECTION_START) && content.contains(MANAGED_SECTION_END) {
        return content.to_string();
    }

    if let Some((frontmatter, body)) = split_frontmatter(content) {
        let body = body.trim_start_matches('\n');
        return format!("{frontmatter}\n{MANAGED_SECTION_START}\n{body}\n{MANAGED_SECTION_END}\n");
    }

    format!("{MANAGED_SECTION_START}\n{content}\n{MANAGED_SECTION_END}\n")
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---\n")?;
    let frontmatter_end = 4 + end + 5;
    Some((&content[..frontmatter_end], &content[frontmatter_end..]))
}

struct ManagedSectionWarning {
    diff: String,
}

struct PushSyncWarning {
    frontmatter: Option<FrontmatterWarning>,
    managed: Option<ManagedSectionWarning>,
    notes: Vec<String>,
}

struct NoteLocationWarning {
    desired_path: PathBuf,
    status: String,
    identifier: String,
}

struct IssueUpdatePlan {
    input: JsonMap<String, Value>,
    updated_keys: Vec<String>,
    notes: Vec<String>,
    will_move_to_done: bool,
}

fn discover_markdown_notes(root: &Path, include_done: bool) -> Vec<PathBuf> {
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

fn discover_markdown_notes_for_issue(
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

fn parse_local_note(path: &Path) -> Result<LocalNote, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {}", path.display(), error))?;
    let (frontmatter, _) = split_frontmatter(&content)
        .ok_or_else(|| "note is missing YAML frontmatter".to_string())?;
    let frontmatter = parse_frontmatter_map(frontmatter)
        .ok_or_else(|| "note frontmatter is not valid YAML".to_string())?;

    let identifier = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| "note file name does not contain an issue identifier".to_string())?
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

fn extract_linear_id_from_frontmatter(frontmatter: &serde_yaml::Mapping) -> Option<String> {
    let value = frontmatter.get(YamlValue::String("linear_id".to_string()))?;
    let value = yaml_string(value)?;
    let value = value.trim();

    if let Some(rest) = value.strip_prefix('[')
        && let Some((label, _)) = rest.split_once(']')
    {
        let label = label.trim();
        if !label.is_empty() {
            return Some(label.to_string());
        }
    }

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn normalize_frontmatter_key(key: &str) -> String {
    match key.trim().to_lowercase().as_str() {
        "labels" | "label" => "tags".to_string(),
        "state" => "status".to_string(),
        other => other.to_string(),
    }
}

fn status_slug(status: &str) -> String {
    status.trim().to_lowercase().replace(' ', "-")
}

fn is_done_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("done"))
        .unwrap_or(false)
}

fn include_done_issue(
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

fn find_issue_note_in_other_status(output_dir: &Path, identifier: &str) -> Option<PathBuf> {
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

fn note_location_warning(
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

fn insert_or_remove_generated_section(
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

fn insert_or_remove_note_location_warning(
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

fn insert_or_remove_push_sync_section(content: &str, warning: Option<&PushSyncWarning>) -> String {
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

fn extract_managed_section_body(content: &str) -> Option<&str> {
    let managed = extract_managed_section(content)?;
    let managed = managed.strip_prefix(MANAGED_SECTION_START)?;
    let managed = managed.strip_suffix(MANAGED_SECTION_END)?;
    Some(managed.trim())
}

fn normalize_managed_section_for_diff(content: &str) -> Vec<String> {
    extract_managed_section_body(content)
        .unwrap_or("")
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim_start().starts_with("*Last synced:"))
        .map(ToString::to_string)
        .collect()
}

fn render_text_diff(local: &[String], remote: &[String]) -> Option<String> {
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

fn managed_section_warning(
    local_content: &str,
    remote_content: &str,
) -> Option<ManagedSectionWarning> {
    let local_lines = normalize_managed_section_for_diff(local_content);
    let remote_lines = normalize_managed_section_for_diff(remote_content);
    render_text_diff(&local_lines, &remote_lines).map(|diff| ManagedSectionWarning { diff })
}

fn push_frontmatter_diff_warning(
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

fn collect_frontmatter_keys(
    left: &serde_yaml::Mapping,
    right: &serde_yaml::Mapping,
    ignored_properties: &[String],
) -> Vec<String> {
    let mut keys = left
        .keys()
        .chain(right.keys())
        .filter_map(|key| match key {
            YamlValue::String(key) => Some(normalize_frontmatter_key(key)),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    keys.retain(|key| {
        key != "ignored_properties"
            && !ignored_properties
                .iter()
                .any(|ignored| normalize_frontmatter_key(ignored) == *key)
    });

    keys
}

fn resolve_force_keys(
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

fn yaml_string(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(value) => Some(value.trim().to_string()),
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn yaml_string_list(value: &YamlValue) -> Option<Vec<String>> {
    match value {
        YamlValue::Sequence(values) => Some(
            values
                .iter()
                .filter_map(yaml_string)
                .map(|value| value.replace(' ', "-"))
                .collect(),
        ),
        YamlValue::String(value) => Some(
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.replace(' ', "-"))
                .collect(),
        ),
        _ => None,
    }
}

fn normalize_project_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let trimmed = trimmed
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
        .unwrap_or(trimmed)
        .trim();

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn override_frontmatter_value(content: &str, key: &str, value: YamlValue) -> String {
    let Some((frontmatter, body)) = split_frontmatter(content) else {
        return content.to_string();
    };
    let Some(mut map) = parse_frontmatter_map(frontmatter) else {
        return content.to_string();
    };
    let key_value = YamlValue::String(key.to_string());
    if !map.contains_key(&key_value) {
        return content.to_string();
    }

    map.insert(key_value, value);
    let yaml = match serde_yaml::to_string(&map) {
        Ok(yaml) => yaml,
        Err(_) => return content.to_string(),
    };

    format!("---\n{}---\n{}", yaml, body)
}

fn final_note_path_after_push(current_path: &Path, status: &str) -> PathBuf {
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

fn write_note_to_path(original_path: &Path, final_path: &Path, content: &str) -> io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_content_keeps_frontmatter_at_top() {
        let content = default_markdown_content(
            "Title",
            "In Progress",
            "Urgent",
            "id-1",
            "",
            "",
            "Project",
            "Desc",
            "https://linear.app/test",
            "2026-04-28 12:00:00",
        );

        assert!(content.content.starts_with("---\n"));
        assert!(content.content.contains(MANAGED_SECTION_START));
        assert!(content.content.contains(MANAGED_SECTION_END));
        assert!(content.content.find(MANAGED_SECTION_START).unwrap() > 0);
    }

    #[test]
    fn template_with_frontmatter_wraps_only_body() {
        let template = r#"---
title: "{{title}}"
status: "{{status}}"
---

{{description_section}}Body line
"#;

        let rendered = render_template(
            template,
            &TemplateContext {
                title: "Title",
                status: "Todo",
                priority: "No priority",
                issue_id: "id-1",
                identifier: "ABC-1",
                url: "https://linear.app/test",
                project: "Project",
                description_section: ">[!info]+ Description\n> Desc\n\n",
                labels_yaml: "",
                gh_yaml: "",
                now: "2026-04-28 12:00:00",
                team_name: "Team",
            },
        );

        assert!(rendered.content.starts_with("---\n"));
        assert!(rendered.content.contains("title: \"Title\""));
        assert!(rendered.content.contains(MANAGED_SECTION_START));
        assert!(rendered.content.contains("Body line"));
        assert!(rendered.content.contains(">[!info]+ Description"));
    }

    #[test]
    fn template_omits_description_section_when_missing() {
        let rendered = render_template(
            "{{description_section}}Body line\n",
            &TemplateContext {
                title: "Title",
                status: "Todo",
                priority: "No priority",
                issue_id: "id-1",
                identifier: "ABC-1",
                url: "https://linear.app/test",
                project: "Project",
                description_section: "",
                labels_yaml: "",
                gh_yaml: "",
                now: "2026-04-28 12:00:00",
                team_name: "Team",
            },
        );

        assert!(rendered.content.contains("Body line"));
        assert!(!rendered.content.contains("Description"));
    }

    #[test]
    fn merge_repairs_note_that_only_has_managed_block() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("note.md");

        fs::write(
            &file_path,
            format!("{MANAGED_SECTION_START}\nold body\n{MANAGED_SECTION_END}\n\nMy notes\n"),
        )
        .unwrap();

        let new_content = r#"---
title: "Title"
status: "Todo"
---

<!-- linear-sync:managed:start -->
new body
<!-- linear-sync:managed:end -->

## My notes
"#;

        let merged = merge_with_existing_note(&file_path, new_content, &[]);

        assert!(merged.content.starts_with("---\n"));
        assert!(merged.content.contains("title: \"Title\""));
        assert!(merged.content.contains("new body"));
        assert!(merged.content.contains("My notes"));
    }

    #[test]
    fn frontmatter_conflict_creates_warning() {
        let existing = concat!(
            "---\n",
            "title: \"Local title\"\n",
            "status: \"Todo\"\n",
            "id: \"ACA-122\"\n",
            "tags:\n",
            "  - local-tag\n",
            "project: \"[[General]]\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\n",
            "old\n",
            "<!-- linear-sync:managed:end -->\n",
        );

        let imported = concat!(
            "---\n",
            "title: \"Imported title\"\n",
            "status: \"Todo\"\n",
            "tags:\n",
            "  - missing-tag\n",
            "project: \"[[Imported Project]]\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\n",
            "new\n",
            "<!-- linear-sync:managed:end -->\n",
        );

        let warning = frontmatter_conflict_warning(existing, imported, &[]).unwrap();
        assert!(
            warning
                .diff
                .contains("~ title: \"Local title\" -> \"Imported title\"")
        );
        assert!(
            warning
                .diff
                .contains("~ project: \"[[General]]\" -> \"[[Imported Project]]\"")
        );
        assert!(warning.diff.contains("- id: \"ACA-122\""));
    }

    #[test]
    fn merge_preserves_existing_frontmatter_keys() {
        let existing = concat!(
            "---\n",
            "title: \"Local title\"\n",
            "id: \"ACA-122\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\nold\n<!-- linear-sync:managed:end -->\n",
        );
        let imported = concat!(
            "---\n",
            "title: \"Imported title\"\n",
            "status: \"Todo\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\nnew\n<!-- linear-sync:managed:end -->\n",
        );

        let merged = merge_frontmatter(existing, imported);
        assert!(merged.contains("id: \"ACA-122\""));
        assert!(merged.starts_with("---\n"));
    }

    #[test]
    fn ignored_properties_are_excluded_from_diff() {
        let template = concat!(
            "---\n",
            "title: \"{{title}}\"\n",
            "ignored_properties: alias, id\n",
            "---\n\n",
            "Body\n"
        );

        let rendered = render_template(
            template,
            &TemplateContext {
                title: "Imported title",
                status: "Todo",
                priority: "No priority",
                issue_id: "id-1",
                identifier: "ABC-1",
                url: "https://linear.app/test",
                project: "Project",
                description_section: ">[!info]+ Description\n> Desc\n\n",
                labels_yaml: "",
                gh_yaml: "",
                now: "2026-04-28 12:00:00",
                team_name: "Team",
            },
        );

        let existing = concat!(
            "---\n",
            "title: \"Local title\"\n",
            "alias: \"Keep me\"\n",
            "id: \"ACA-122\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\nold\n<!-- linear-sync:managed:end -->\n",
        );

        let warning =
            frontmatter_conflict_warning(existing, &rendered.content, &rendered.ignored_properties)
                .unwrap();
        assert!(
            warning
                .diff
                .contains("~ title: \"Local title\" -> \"Imported title\"")
        );
        assert!(!warning.diff.contains("alias"));
        assert!(!warning.diff.contains("id: \"ACA-122\""));
    }

    #[test]
    fn push_frontmatter_diff_uses_all_non_ignored_keys() {
        let local = concat!(
            "---\n",
            "title: \"Local title\"\n",
            "priority: 2\n",
            "ignored_properties: aliases\n",
            "aliases: keep-me\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\n",
            "Body\n",
            "<!-- linear-sync:managed:end -->\n",
        );
        let remote = concat!(
            "---\n",
            "title: \"Remote title\"\n",
            "priority: 1\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\n",
            "Body\n",
            "<!-- linear-sync:managed:end -->\n",
        );

        let warning =
            push_frontmatter_diff_warning(local, remote, &["aliases".to_string()]).unwrap();
        assert!(
            warning
                .diff
                .contains("~ title: \"Local title\" -> \"Remote title\"")
        );
        assert!(warning.diff.contains("~ priority: 2 -> 1"));
        assert!(!warning.diff.contains("aliases"));
    }

    #[test]
    fn managed_section_diff_ignores_last_synced_line() {
        let local = concat!(
            "---\n",
            "title: \"Title\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\n",
            "Content\n",
            "---\n",
            "*Last synced: 2026-04-28 12:00:00*\n",
            "<!-- linear-sync:managed:end -->\n",
        );
        let remote = concat!(
            "---\n",
            "title: \"Title\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\n",
            "Content\n",
            "---\n",
            "*Last synced: 2026-04-29 12:00:00*\n",
            "<!-- linear-sync:managed:end -->\n",
        );

        assert!(managed_section_warning(local, remote).is_none());
    }

    #[test]
    fn pull_location_warning_is_inserted_after_frontmatter() {
        let content = concat!(
            "---\n",
            "title: \"Title\"\n",
            "---\n\n",
            "<!-- linear-sync:managed:start -->\n",
            "Body\n",
            "<!-- linear-sync:managed:end -->\n",
        );

        let warning = NoteLocationWarning {
            desired_path: PathBuf::from("linear-issues/done/ABC-1.md"),
            status: "Done".to_string(),
            identifier: "ABC-1".to_string(),
        };

        let updated = insert_or_remove_note_location_warning(content, Some(&warning));
        assert!(updated.contains("Move this note in Obsidian"));
        assert!(updated.contains("linear-issues/done/ABC-1.md"));
    }
}

fn extract_managed_section(content: &str) -> Option<&str> {
    extract_section(content, MANAGED_SECTION_START, MANAGED_SECTION_END)
}

fn extract_section<'a>(content: &'a str, start_marker: &str, end_marker: &str) -> Option<&'a str> {
    let start = content.find(start_marker)?;
    let after_start = start + start_marker.len();
    let end_relative = content[after_start..].find(end_marker)?;
    let end = after_start + end_relative + end_marker.len();
    Some(&content[start..end])
}

fn remove_section(content: &str, start_marker: &str, end_marker: &str) -> String {
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

fn extract_user_content(content: &str) -> Option<String> {
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

struct FrontmatterWarning {
    diff: String,
    keys: Vec<String>,
}

fn print_colored_diff(diff: &str) {
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("- ") {
            println!("  {red}- {rest}{reset}", red = ANSI_RED, reset = ANSI_RESET);
        } else if let Some(rest) = line.strip_prefix("+ ") {
            println!(
                "  {green}+ {rest}{reset}",
                green = ANSI_GREEN,
                reset = ANSI_RESET
            );
        } else if let Some(rest) = line.strip_prefix("~ ") {
            println!(
                "  {blue}~ {rest}{reset}",
                blue = ANSI_BLUE,
                reset = ANSI_RESET
            );
        } else {
            println!("  {line}");
        }
    }
}

fn print_push_diff(
    use_delta: bool,
    identifier: &str,
    diff_label: &str,
    file_path: &Path,
    diff: &str,
) {
    if use_delta {
        print_delta_output(&format_delta_patch(identifier, diff_label, file_path, diff));
    } else {
        print_colored_diff(diff);
    }
}

fn format_delta_patch(identifier: &str, team_name: &str, file_path: &Path, diff: &str) -> String {
    let body = diff
        .lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("- ") {
                format!("-{rest}")
            } else if let Some(rest) = line.strip_prefix("+ ") {
                format!("+{rest}")
            } else if let Some(rest) = line.strip_prefix("~ ") {
                format!(" {rest}")
            } else {
                format!(" {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ {identifier} ({team_name}) @@\n{body}\n\n",
        path = file_path.display(),
    )
}

fn print_delta_output(delta_output: &str) {
    if print_diff_with_delta(delta_output).is_none() {
        print_yaml_style_diff_from_delta_output(delta_output);
    }
}

fn print_yaml_style_diff_from_delta_output(delta_output: &str) {
    let mut current_header: Option<String> = None;
    let mut current_diff_lines = Vec::new();

    for line in delta_output.lines() {
        if let Some(header) = parse_delta_patch_header(line) {
            if let Some(previous_header) = current_header.replace(header) {
                println!("{previous_header}");
                print_colored_diff(&current_diff_lines.join("\n"));
                current_diff_lines.clear();
            }
            continue;
        }

        if line.starts_with("diff --git ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.trim().is_empty()
        {
            continue;
        }

        current_diff_lines.push(normalize_delta_fallback_line(line));
    }

    if let Some(header) = current_header {
        println!("{header}");
        print_colored_diff(&current_diff_lines.join("\n"));
    }
}

fn parse_delta_patch_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("@@ ")?;
    let header = rest.strip_suffix(" @@")?;
    Some(header.to_string())
}

fn normalize_delta_fallback_line(line: &str) -> String {
    if let Some(rest) = line.strip_prefix('-') {
        format!("- {}", rest)
    } else if let Some(rest) = line.strip_prefix('+') {
        format!("+ {}", rest)
    } else {
        line.trim_start().to_string()
    }
}

fn print_diff_with_delta(diff: &str) -> Option<()> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new("delta")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .spawn()
        .ok()?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(diff.as_bytes()).ok()?;
    }

    let status = child.wait().ok()?;
    if status.success() { Some(()) } else { None }
}

struct MergeResult {
    content: String,
    warning: Option<FrontmatterWarning>,
}

fn merge_with_existing_note(
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

fn merge_frontmatter(existing: &str, new_content: &str) -> String {
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

fn insert_or_remove_conflict_section(
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

fn frontmatter_conflict_warning(
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

fn extract_ignored_properties(content: &str) -> Vec<String> {
    let Some((frontmatter, _)) = split_frontmatter(content) else {
        return Vec::new();
    };
    let Some(map) = parse_frontmatter_map(frontmatter) else {
        return Vec::new();
    };

    let ignored = map.get(YamlValue::String("ignored_properties".to_string()));
    match ignored {
        Some(YamlValue::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        Some(YamlValue::Sequence(values)) => values
            .iter()
            .filter_map(|value| match value {
                YamlValue::String(value) => Some(value.trim().to_string()),
                _ => None,
            })
            .filter(|value| !value.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_frontmatter_map(frontmatter: &str) -> Option<serde_yaml::Mapping> {
    let body = frontmatter.strip_prefix("---\n")?;
    let body = body
        .strip_suffix("---\n")
        .or_else(|| body.strip_suffix("\n---"))
        .or_else(|| body.strip_suffix("---"))?;
    let yaml = serde_yaml::from_str::<YamlValue>(body.trim()).ok()?;
    yaml.as_mapping().cloned()
}

fn render_yaml_value_diff(prefix: char, key: &str, value: &YamlValue) -> Vec<String> {
    match value {
        YamlValue::Sequence(sequence) => {
            let mut lines = vec![format!("{prefix} {key}:")];
            for item in sequence {
                lines.push(format!("{prefix}   - {}", yaml_scalar_for_diff(item)));
            }
            lines
        }
        _ => vec![format!("{prefix} {key}: {}", yaml_scalar_for_diff(value))],
    }
}

fn render_modified_yaml_value_diff(key: &str, old: &YamlValue, new: &YamlValue) -> Vec<String> {
    match (old, new) {
        (YamlValue::Sequence(old_seq), YamlValue::Sequence(new_seq)) => {
            let old_items = old_seq.iter().map(yaml_scalar_for_diff).collect::<Vec<_>>();
            let new_items = new_seq.iter().map(yaml_scalar_for_diff).collect::<Vec<_>>();

            let removed = old_items
                .iter()
                .filter(|item| !new_items.contains(item))
                .cloned()
                .collect::<Vec<_>>();
            let added = new_items
                .iter()
                .filter(|item| !old_items.contains(item))
                .cloned()
                .collect::<Vec<_>>();

            let mut lines = vec![format!("~ {key}:")];
            for item in removed {
                lines.push(format!("-   - {item}"));
            }
            for item in added {
                lines.push(format!("+   - {item}"));
            }
            lines
        }
        _ => vec![format!(
            "~ {key}: {} -> {}",
            yaml_scalar_for_diff(old),
            yaml_scalar_for_diff(new)
        )],
    }
}

fn yaml_scalar_for_diff(value: &YamlValue) -> String {
    match value {
        YamlValue::String(text) => format!("\"{}\"", text),
        _ => serde_yaml::to_string(value)
            .unwrap_or_else(|_| "<unrenderable>".to_string())
            .trim()
            .to_string(),
    }
}

fn fetch_required_issue(client: &Client, api_key: &str, identifier: &str) -> RemoteIssue {
    match fetch_remote_issue_by_identifier(client, api_key, identifier) {
        Ok(Some(issue)) => issue,
        Ok(None) => {
            eprintln!("❌ Error: Could not find Linear issue `{}`.", identifier);
            process::exit(1);
        }
        Err(error) => {
            eprintln!(
                "❌ Error: Failed to fetch Linear issue `{}`: {}",
                identifier, error
            );
            process::exit(1);
        }
    }
}

fn file_path_for_issue(output_dir: &Path, status: &str, identifier: &str) -> PathBuf {
    output_dir
        .join(status_slug(status))
        .join(format!("{}.md", identifier))
}

fn graphql_request(client: &Client, api_key: &str, query: &str, variables: Value) -> Value {
    let response = client
        .post(LINEAR_API_URL)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .expect("Failed to send request to Linear API");

    if response.status().is_success() {
        response.json().expect("Failed to parse JSON")
    } else {
        eprintln!("❌ API Request Failed: {:?}", response.status());
        eprintln!("Response body: {:?}", response.text().unwrap_or_default());
        process::exit(1);
    }
}

fn default_output_root() -> PathBuf {
    PathBuf::from(DEFAULT_OUTPUT_ROOT)
}

fn default_output_root_for_all_teams(merge_all_teams: bool) -> PathBuf {
    if merge_all_teams {
        default_output_root().join("all-teams")
    } else {
        default_output_root()
    }
}

fn default_output_dir_for_team(team_name: &str) -> PathBuf {
    default_output_root().join(slugify_team_name(team_name))
}

fn slugify_team_name(team_name: &str) -> String {
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
