use chrono::Utc;
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";
const DEFAULT_OUTPUT_ROOT: &str = "linear-issues";
const ALL_TEAMS_OPTION: &str = "ALL TEAMS";

#[derive(Parser)]
#[command(
    name = "linear-sync",
    version = "1.0",
    about = "Syncs Linear issues with local Markdown files"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Pulls active issues from Linear and generates markdown files
    Pull {
        /// The UUID of the Linear Team. If omitted, pulls from all teams.
        #[arg(short, long)]
        team_id: Option<String>,
        /// The path to your Obsidian vault directory for issues.
        /// Defaults to linear-issues/<team-name>, linear-issues/<each-team-name>,
        /// or linear-issues/all-teams when merging all teams.
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
        /// Merge issues from all teams into a single subdirectory.
        #[arg(short = 'm', long)]
        merge_all_teams: bool,
        /// Interactively confirm the team selection, merge behavior, and output directory.
        #[arg(short = 'c', long)]
        confirm: bool,
    },
    /// Pushes local status updates back to Linear
    Push {
        /// The path to your Obsidian vault directory
        #[arg(short, long)]
        input_dir: PathBuf,
    },
}

#[derive(Clone, Debug)]
struct TeamInfo {
    id: String,
    name: String,
}

fn main() {
    dotenv().ok();

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
            output_dir,
            merge_all_teams,
            confirm,
        } => {
            pull_command(
                &client,
                &api_key,
                team_id.clone(),
                output_dir.clone(),
                *merge_all_teams,
                *confirm,
            );
        }
        Commands::Push { input_dir } => {
            println!("Push command initiated for {:?}", input_dir);
        }
    }
}

fn pull_command(
    client: &Client,
    api_key: &str,
    team_id: Option<String>,
    output_dir: Option<PathBuf>,
    merge_all_teams: bool,
    confirm: bool,
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

    match selection {
        PullSelection::SingleTeam { team, output_dir } => {
            pull_issues(client, api_key, &team, &output_dir);
        }
        PullSelection::AllTeams {
            root_output_dir,
            merge_all_teams,
        } => {
            for team in teams {
                let team_output_dir = if merge_all_teams {
                    root_output_dir.clone()
                } else {
                    root_output_dir.join(slugify_team_name(&team.name))
                };
                pull_issues(client, api_key, &team, &team_output_dir);
            }
        }
    }
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

fn pull_issues(client: &Client, api_key: &str, team: &TeamInfo, output_dir: &PathBuf) {
    let query = r#"
    query GetTeamIssues($teamId: String!) {
      team(id: $teamId) {
        issues(filter: { state: { type: { in: ["started", "unstarted"] } } }) {
          nodes {
            id
            identifier
            title
            url
            description
            state {
              name
            }
            labels {
              nodes {
                name
              }
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

    if !output_dir.exists() {
        fs::create_dir_all(output_dir).expect("Failed to create output directory");
    }

    let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    for issue in issues {
        let identifier = issue["identifier"].as_str().unwrap_or("UNKNOWN");
        let title = issue["title"].as_str().unwrap_or("No Title");
        let status = issue["state"]["name"].as_str().unwrap_or("Todo");
        let url = issue["url"].as_str().unwrap_or("");
        let issue_id = issue["id"].as_str().unwrap_or("");
        let description = issue["description"]
            .as_str()
            .unwrap_or("No description provided");

        let formatted_description = description.replace("\n", "\n> ");

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

        let markdown_content = format!(
            r#"---
title: "{title}"
status: "{status}"
priority: 0
linear_id: "{issue_id}"
{labels_yaml}{gh_yaml}---

>[!info] Description
> {formatted_description}

[Open in Linear]({url})

---
*Last synced: {now}*
"#
        );

        let safe_status = status.to_lowercase().replace(' ', "-");
        let status_dir = output_dir.join(&safe_status);
        if !status_dir.exists() {
            if let Err(e) = fs::create_dir_all(&status_dir) {
                eprintln!("⚠️ Failed to create directory {:?}: {}", status_dir, e);
            }
        }

        let file_path = status_dir.join(format!("{}.md", identifier));
        if let Err(e) = fs::write(&file_path, &markdown_content) {
            eprintln!("⚠️ Failed to write file {}.md: {}", identifier, e);
        } else {
            println!(
                "✅ Successfully synced: {} ({}) to {}",
                identifier,
                team.name,
                output_dir.display()
            );
        }
    }
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
