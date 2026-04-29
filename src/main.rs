use chrono::Utc;
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use reqwest::blocking::Client;
use serde_json::{Value, json};
use serde_yaml::Value as YamlValue;
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
        #[arg(long)]
        team_id: Option<String>,
        /// The path to your Obsidian vault directory for issues.
        /// Defaults to linear-issues/<team-name>, linear-issues/<each-team-name>,
        /// or linear-issues/all-teams when merging all teams.
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
        /// Path to a Markdown template file used to structure created notes.
        #[arg(short = 'p', long)]
        template: Option<PathBuf>,
        /// Merge issues from all teams into a single subdirectory.
        #[arg(short = 'm', long)]
        merge_all_teams: bool,
        /// Interactively confirm the team selection, merge behavior, and output directory.
        #[arg(short = 'c', long)]
        confirm: bool,
        /// Overwrite the entire note instead of only updating the managed section.
        #[arg(long)]
        force: bool,
        /// Preview note changes without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Use YAML-style diff output instead of delta rendering.
        #[arg(long)]
        no_delta: bool,
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
            output_dir,
            template,
            merge_all_teams,
            confirm,
            force,
            dry_run,
            no_delta,
        } => {
            pull_command(
                &client,
                &api_key,
                team_id.clone(),
                output_dir.clone(),
                template.clone(),
                *merge_all_teams,
                *confirm,
                *force,
                !*no_delta,
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
    template_path: Option<PathBuf>,
    merge_all_teams: bool,
    confirm: bool,
    force: bool,
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

    match selection {
        PullSelection::SingleTeam { team, output_dir } => {
            let stats = pull_issues(
                client,
                api_key,
                &team,
                &output_dir,
                template.as_deref(),
                force,
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
                    &team,
                    &team_output_dir,
                    template.as_deref(),
                    force,
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
            println!(
                "Imported {} notes ({} warnings).",
                total.imported, total.warnings
            );
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

#[derive(Default)]
struct PullStats {
    imported: usize,
    warnings: usize,
    delta_output: String,
}

fn pull_issues(
    client: &Client,
    api_key: &str,
    team: &TeamInfo,
    output_dir: &PathBuf,
    template: Option<&str>,
    force: bool,
    use_delta: bool,
) -> PullStats {
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

    if !output_dir.exists() {
        fs::create_dir_all(output_dir).expect("Failed to create output directory");
    }

    let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let mut stats = PullStats::default();

    for issue in issues {
        let identifier = issue["identifier"].as_str().unwrap_or("UNKNOWN");
        let title = issue["title"].as_str().unwrap_or("No Title");
        let status = issue["state"]["name"].as_str().unwrap_or("Todo");
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

        let issue_file_path = file_path_for_issue(output_dir, status, identifier);
        let merge_result = if force {
            MergeResult {
                content: markdown_content,
                warning: None,
            }
        } else {
            merge_with_existing_note(
                &issue_file_path,
                &markdown_content,
                &rendered_note.ignored_properties,
            )
        };
        let markdown_content = merge_result.content;

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
            stats.imported += 1;
            if let Some(warning) = merge_result.warning {
                stats.warnings += 1;
                println!(
                    "{yellow}⚠ Frontmatter conflict:{reset} {} ({}) -> {}",
                    identifier,
                    team.name,
                    file_path.display(),
                    yellow = ANSI_YELLOW,
                    reset = ANSI_RESET,
                );
                if use_delta {
                    stats.delta_output.push_str(&format_delta_patch(
                        identifier,
                        &team.name,
                        &file_path,
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

fn default_markdown_content(
    title: &str,
    status: &str,
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
priority: 0
linear_id: "{issue_id}"
{labels_yaml}{gh_yaml}project: "[[{project}]]"
---

{MANAGED_SECTION_START}
{description_section}[Open in Linear]({url})

---
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_content_keeps_frontmatter_at_top() {
        let content = default_markdown_content(
            "Title",
            "In Progress",
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

fn file_path_for_issue(output_dir: &Path, status: &str, identifier: &str) -> PathBuf {
    let safe_status = status.to_lowercase().replace(' ', "-");
    output_dir
        .join(safe_status)
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
