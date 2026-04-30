mod cli;
mod linear {
    pub(crate) mod client;
    pub(crate) mod models;
}
mod notes {
    pub(crate) mod discovery;
    pub(crate) mod frontmatter;
    pub(crate) mod paths;
    pub(crate) mod reconcile;
    pub(crate) mod render;
    pub(crate) mod sections;
}
mod app {
    pub(crate) mod pull;
    pub(crate) mod push;
}
mod output {
    pub(crate) mod diff;
}

use crate::cli::{Cli, Commands, ForceSelection, parse_force_selection};
use crate::app::pull::pull_command;
use crate::app::push::push_command;
use crate::linear::client::{
    fetch_priority_values, fetch_project_by_name, fetch_remote_issue_by_id,
    fetch_remote_issue_for_note, resolve_label, resolve_state, update_linear_issue,
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
use crate::notes::render::{
    initialize_installed_template_path, load_template, render_remote_issue_note,
};
use crate::notes::sections::{
    PushSyncWarning, insert_or_remove_note_location_warning,
    insert_or_remove_push_sync_section, split_frontmatter,
};
use crate::output::diff::{ANSI_BLUE, ANSI_RED, ANSI_RESET, ANSI_YELLOW, print_push_diff};
use chrono::Utc;
use clap::Parser;
use dotenvy::dotenv;
use reqwest::blocking::Client;
use serde_json::{Map as JsonMap, Value, json};
use serde_yaml::Value as YamlValue;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::reconcile::{
        frontmatter_conflict_warning, merge_frontmatter, merge_with_existing_note,
    };
    use crate::notes::render::{TemplateContext, default_markdown_content, render_template};
    use crate::notes::sections::{MANAGED_SECTION_END, MANAGED_SECTION_START};

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

        let warning = crate::notes::sections::NoteLocationWarning {
            desired_path: PathBuf::from("linear-issues/done/ABC-1.md"),
            status: "Done".to_string(),
            identifier: "ABC-1".to_string(),
        };

        let updated = insert_or_remove_note_location_warning(content, Some(&warning));
        assert!(updated.contains("Move this note in Obsidian"));
        assert!(updated.contains("linear-issues/done/ABC-1.md"));
    }
}


