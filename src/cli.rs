use clap::{Parser, Subcommand};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "linear-sync",
    version = "1.0",
    about = "Syncs Linear issues with local Markdown files"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Pulls active issues from Linear and generates markdown files
    Pull {
        /// The UUID of the Linear Team. If omitted, pulls from all teams.
        #[arg(long)]
        team_id: Option<String>,
        /// Pull only a single issue by identifier, such as ACA-125.
        #[arg(long)]
        issue_id: Option<String>,
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
        /// Include Done issues that are already in the done subdirectory or do not yet exist locally.
        /// Issues transitioning to Done are always processed.
        #[arg(long)]
        include_done: bool,
        /// Preview note changes without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Use YAML-style diff output instead of delta rendering.
        #[arg(long)]
        no_delta: bool,
    },
    /// Pushes local note metadata back to Linear
    Push {
        /// The path to your Obsidian vault directory
        #[arg(short, long)]
        input_dir: PathBuf,
        /// Push only a single issue by identifier, such as ACA-125.
        #[arg(long)]
        issue_id: Option<String>,
        /// Path to a Markdown template file used to diff managed note content.
        #[arg(short = 'p', long)]
        template: Option<PathBuf>,
        /// Push selected frontmatter properties back to Linear.
        /// Pass without a value to sync all supported differing properties.
        /// Example: --force=title,status,priority
        #[arg(
            long,
            num_args = 0..,
            value_delimiter = ',',
            require_equals = true,
            default_missing_value = "__all__"
        )]
        force: Vec<String>,
        /// Include notes already under a done subdirectory.
        /// Notes transitioning to Done are always processed.
        #[arg(long)]
        include_done: bool,
        /// Preview push changes without updating Linear or editing local notes.
        #[arg(long)]
        dry_run: bool,
        /// Use YAML-style diff output instead of delta rendering.
        #[arg(long)]
        no_delta: bool,
    },
}

pub(crate) enum ForceSelection {
    None,
    All,
    Selected(BTreeSet<String>),
}

pub(crate) fn parse_force_selection(values: &[String]) -> ForceSelection {
    if values.is_empty() {
        return ForceSelection::None;
    }

    if values.iter().any(|value| value == "__all__") {
        return ForceSelection::All;
    }

    ForceSelection::Selected(
        values
            .iter()
            .map(|value| crate::normalize_frontmatter_key(value))
            .filter(|value| !value.is_empty())
            .collect(),
    )
}
