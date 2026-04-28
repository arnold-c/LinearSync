use chrono::Utc;
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use reqwest::blocking::Client;
use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process; // Import process to exit gracefully

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
        /// The UUID of the Linear Team
        #[arg(short, long)]
        team_id: String,
        /// The path to your Obsidian vault directory for issues
        #[arg(short, long)]
        output_dir: PathBuf,
    },
    /// Pushes local status updates back to Linear
    Push {
        /// The path to your Obsidian vault directory
        #[arg(short, long)]
        input_dir: PathBuf,
    },
}

fn main() {
    // 1. Attempt to load the .env file.
    // If it exists, it loads those variables into the environment.
    // If it doesn't exist, .ok() silently ignores the error and relies on the OS environment.
    dotenv().ok();

    let cli = Cli::parse();

    // 2. Safely check for the environment variable without panicking
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

            // Exit the program with a status code of 1 (indicating an error occurred)
            process::exit(1);
        }
    };

    let client = Client::new();

    match &cli.command {
        Commands::Pull {
            team_id,
            output_dir,
        } => {
            pull_issues(&client, &api_key, team_id, output_dir);
        }
        Commands::Push { input_dir } => {
            println!("Push command initiated for {:?}", input_dir);
        }
    }
}

fn pull_issues(client: &Client, api_key: &str, team_id: &str, output_dir: &PathBuf) {
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

    let variables = json!({ "teamId": team_id });

    let response = client
        .post("https://api.linear.app/graphql")
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .expect("Failed to send request to Linear API");

    if response.status().is_success() {
        let res_json: serde_json::Value = response.json().expect("Failed to parse JSON");

        // Safely check if the nodes array exists
        let issues = match res_json["data"]["team"]["issues"]["nodes"].as_array() {
            Some(arr) => arr,
            None => {
                eprintln!(
                    "❌ Error: Could not find any issues. Please verify your team ID is correct."
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
                            // Replace spaces with hyphens so Obsidian parses them correctly
                            let clean_name = name.replace(" ", "-");
                            labels_yaml.push_str(&format!("  - {}\n", clean_name));
                        }
                    }
                }
            }

            // 3. Extract GitHub Attachments
            let mut gh_yaml = String::new();
            if let Some(attachments_nodes) = issue["attachments"]["nodes"].as_array() {
                let mut temp_gh = String::from("github_links:\n");
                let mut has_links = false;

                for attachment in attachments_nodes {
                    if let Some(attachment_url) = attachment["url"].as_str() {
                        // Only grab URLs pointing to PRs or Issues
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

            let safe_status = status.to_lowercase().replace(" ", "-");
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
                println!("✅ Successfully synced: {} to {}", identifier, safe_status);
            }
        }
    } else {
        eprintln!("❌ API Request Failed: {:?}", response.status());
        eprintln!("Response body: {:?}", response.text().unwrap_or_default());
        process::exit(1);
    }
}
// use chrono::Utc;
// use clap::{Parser, Subcommand};
// use reqwest::blocking::Client;
// use serde_json::json;
// use std::env;
// use std::fs;
// use std::path::PathBuf;
//
// #[derive(Parser)]
// #[command(
//     name = "linear-sync",
//     version = "1.0",
//     about = "Syncs Linear issues with local Markdown files"
// )]
// struct Cli {
//     #[command(subcommand)]
//     command: Commands,
// }
//
// #[derive(Subcommand)]
// enum Commands {
//     /// Pulls active issues from Linear and generates markdown files
//     Pull {
//         /// The UUID of the Linear Team
//         #[arg(short, long)]
//         team_id: String,
//         /// The path to your Obsidian vault directory for issues
//         #[arg(short, long)]
//         output_dir: PathBuf,
//     },
//     /// Pushes local status updates back to Linear
//     Push {
//         /// The path to your Obsidian vault directory
//         #[arg(short, long)]
//         input_dir: PathBuf,
//     },
// }
//
// fn main() {
//     let cli = Cli::parse();
//
//     // Expect the API key to be set in the shell environment
//     let api_key =
//         env::var("LINEAR_API_KEY").expect("LINEAR_API_KEY environment variable is not set");
//
//     let client = Client::new();
//
//     match &cli.command {
//         Commands::Pull {
//             team_id,
//             output_dir,
//         } => {
//             pull_issues(&client, &api_key, team_id, output_dir);
//         }
//         Commands::Push { input_dir } => {
//             println!("Push command initiated for {:?}", input_dir);
//             // Implementation for parsing local frontmatter and pushing via GraphQL mutation
//             // push_status_updates(&client, &api_key, input_dir);
//         }
//     }
// }
//
// fn pull_issues(client: &Client, api_key: &str, team_id: &str, output_dir: &PathBuf) {
//     let query = r#"
//     query GetTeamIssues($teamId: String!) {
//       team(id: $teamId) {
//         issues(filter: { state: { type: { in: ["started", "unstarted"] } } }) {
//           nodes {
//             id
//             identifier
//             title
//             url
//             state {
//               name
//             }
//           }
//         }
//       }
//     }
//     "#;
//
//     let variables = json!({ "teamId": team_id });
//
//     let response = client
//         .post("https://api.linear.app/graphql")
//         .header("Authorization", api_key)
//         .header("Content-Type", "application/json")
//         .json(&json!({ "query": query, "variables": variables }))
//         .send()
//         .expect("Failed to send request");
//
//     if response.status().is_success() {
//         let res_json: serde_json::Value = response.json().expect("Failed to parse JSON");
//
//         let issues = res_json["data"]["team"]["issues"]["nodes"]
//             .as_array()
//             .expect("Could not parse issues array");
//
//         if !output_dir.exists() {
//             fs::create_dir_all(output_dir).expect("Failed to create output directory");
//         }
//
//         let now = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
//
//         for issue in issues {
//             let identifier = issue["identifier"].as_str().unwrap_or("UNKNOWN");
//             let title = issue["title"].as_str().unwrap_or("No Title");
//             let status = issue["state"]["name"].as_str().unwrap_or("Todo");
//             let url = issue["url"].as_str().unwrap_or("");
//             let issue_id = issue["id"].as_str().unwrap_or("");
//
//             // Format exactly as TaskNotes requires
//             let markdown_content = format!(
//                 r#"---
// status: "{status}"
// priority: 0
// linear_id: "{issue_id}"
// ---
// # {title}
//
// [Open in Linear]({url})
//
// ---
// *Last synced: {now}*
// "#
//             );
//
//             let file_path = output_dir.join(format!("{}.md", identifier));
//             fs::write(&file_path, markdown_content).expect("Unable to write file");
//             println!("Successfully synced: {}", identifier);
//         }
//     } else {
//         eprintln!("API Request Failed: {:?}", response.status());
//         eprintln!("Response body: {:?}", response.text().unwrap());
//     }
// }
