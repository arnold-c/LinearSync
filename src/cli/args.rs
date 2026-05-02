use clap::{ArgAction, Args, Parser, Subcommand};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "linear-sync",
    version = "1.0",
    about = "Syncs Linear issues with local Markdown files"
)]
pub(crate) struct Cli {
    /// Path to a TOML config file containing sync profiles.
    #[arg(long, global = true)]
    pub(crate) config: Option<PathBuf>,
    /// One or more profile names from the config file.
    /// Use `all` or omit the flag to run all configured profiles.
    #[arg(long, global = true, value_delimiter = ',')]
    pub(crate) profile: Vec<String>,
    /// Path to an env file to read before resolving LINEAR_API_KEY.
    #[arg(short = 'e', long, global = true)]
    pub(crate) env_file: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Pulls active issues from Linear and generates markdown files
    Pull(PullArgs),
    /// Pushes local note metadata back to Linear
    Push(PushArgs),
}

#[derive(Args, Clone, Debug, Default)]
pub(crate) struct PullArgs {
    /// The UUID of the Linear Team. If omitted, pulls from all teams.
    #[arg(long)]
    pub(crate) team_id: Option<String>,
    /// Pull only a single issue by identifier, such as ACA-125.
    #[arg(long)]
    pub(crate) issue_id: Option<String>,
    /// The path to your Obsidian vault directory for issues.
    /// Defaults to linear-issues/<team-name>, linear-issues/<each-team-name>,
    /// or linear-issues/all-teams when merging all teams.
    #[arg(short, long)]
    pub(crate) output_dir: Option<PathBuf>,
    /// Path to a Markdown template file used to structure created notes.
    #[arg(short = 'p', long)]
    pub(crate) template: Option<PathBuf>,
    /// Merge issues from all teams into a single subdirectory.
    #[arg(
        short = 'm',
        long,
        action = ArgAction::SetTrue,
        overrides_with = "separate_team_dirs"
    )]
    merge_all_teams: bool,
    /// Keep separate team subdirectories when pulling all teams.
    #[arg(
        long,
        action = ArgAction::SetTrue,
        overrides_with = "merge_all_teams"
    )]
    separate_team_dirs: bool,
    /// Interactively confirm the team selection, merge behavior, and output directory.
    #[arg(short = 'c', long, action = ArgAction::SetTrue, overrides_with = "no_confirm")]
    confirm: bool,
    /// Skip the interactive confirmation flow.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "confirm")]
    no_confirm: bool,
    /// Overwrite the entire note instead of only updating the managed section.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "no_force")]
    force: bool,
    /// Preserve the existing note body outside the managed section.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "force")]
    no_force: bool,
    /// Include Done issues that are already in the done subdirectory or do not yet exist locally.
    /// Issues transitioning to Done are always processed.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "skip_done")]
    include_done: bool,
    /// Skip Done issues unless they need to move between active and done locations.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "include_done")]
    skip_done: bool,
    /// Preview note changes without writing files.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "no_dry_run")]
    dry_run: bool,
    /// Write note changes instead of previewing them.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "dry_run")]
    no_dry_run: bool,
    /// Use delta rendering for diff output.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "no_delta")]
    delta: bool,
    /// Use YAML-style diff output instead of delta rendering.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "delta")]
    no_delta: bool,
}

impl PullArgs {
    pub(crate) fn merge_all_teams_override(&self) -> Option<bool> {
        bool_override(self.merge_all_teams, self.separate_team_dirs)
    }

    pub(crate) fn confirm_override(&self) -> Option<bool> {
        bool_override(self.confirm, self.no_confirm)
    }

    pub(crate) fn force_override(&self) -> Option<bool> {
        bool_override(self.force, self.no_force)
    }

    pub(crate) fn include_done_override(&self) -> Option<bool> {
        bool_override(self.include_done, self.skip_done)
    }

    pub(crate) fn dry_run_override(&self) -> Option<bool> {
        bool_override(self.dry_run, self.no_dry_run)
    }

    pub(crate) fn delta_override(&self) -> Option<bool> {
        bool_override(self.delta, self.no_delta)
    }
}

#[derive(Args, Clone, Debug, Default)]
pub(crate) struct PushArgs {
    /// The path to your Obsidian vault directory
    #[arg(short, long)]
    pub(crate) input_dir: Option<PathBuf>,
    /// Push only a single issue by identifier, such as ACA-125.
    #[arg(long)]
    pub(crate) issue_id: Option<String>,
    /// Path to a Markdown template file used to diff managed note content.
    #[arg(short = 'p', long)]
    pub(crate) template: Option<PathBuf>,
    /// Push selected frontmatter properties back to Linear.
    /// Pass without a value to sync all supported differing properties.
    /// Example: --force=title,status,priority
    #[arg(
        long,
        num_args = 0..,
        value_delimiter = ',',
        require_equals = true,
        default_missing_value = "__all__",
        overrides_with = "no_force"
    )]
    force: Vec<String>,
    /// Do not push frontmatter changes to Linear.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "force")]
    no_force: bool,
    /// Include notes already under a done subdirectory.
    /// Notes transitioning to Done are always processed.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "skip_done")]
    include_done: bool,
    /// Skip notes already under a done subdirectory unless they are transitioning.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "include_done")]
    skip_done: bool,
    /// Preview push changes without updating Linear or editing local notes.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "no_dry_run")]
    dry_run: bool,
    /// Apply push changes instead of previewing them.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "dry_run")]
    no_dry_run: bool,
    /// Use delta rendering for diff output.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "no_delta")]
    delta: bool,
    /// Use YAML-style diff output instead of delta rendering.
    #[arg(long, action = ArgAction::SetTrue, overrides_with = "delta")]
    no_delta: bool,
}

impl PushArgs {
    pub(crate) fn force_override(&self) -> Option<ForceSelection> {
        if self.no_force {
            Some(ForceSelection::None)
        } else if self.force.is_empty() {
            None
        } else {
            Some(parse_force_selection(&self.force))
        }
    }

    pub(crate) fn include_done_override(&self) -> Option<bool> {
        bool_override(self.include_done, self.skip_done)
    }

    pub(crate) fn dry_run_override(&self) -> Option<bool> {
        bool_override(self.dry_run, self.no_dry_run)
    }

    pub(crate) fn delta_override(&self) -> Option<bool> {
        bool_override(self.delta, self.no_delta)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ForceSelection {
    None,
    All,
    Selected(BTreeSet<String>),
}

pub(crate) fn parse_force_selection(values: &[String]) -> ForceSelection {
    if values.is_empty() {
        return ForceSelection::None;
    }

    if values
        .iter()
        .any(|value| value == "__all__" || value.eq_ignore_ascii_case("all"))
    {
        return ForceSelection::All;
    }

    ForceSelection::Selected(
        values
            .iter()
            .map(|value| crate::notes::frontmatter::normalize_frontmatter_key(value))
            .filter(|value| !value.is_empty())
            .collect(),
    )
}

fn bool_override(enabled_flag: bool, disabled_flag: bool) -> Option<bool> {
    match (enabled_flag, disabled_flag) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_file_before_subcommand() {
        let cli = Cli::try_parse_from(["linear-sync", "--env-file", "acme.env", "pull"])
            .expect("expected CLI to parse");

        assert_eq!(cli.env_file, Some(PathBuf::from("acme.env")));
        assert!(matches!(cli.command, Commands::Pull(_)));
    }

    #[test]
    fn parses_env_file_after_subcommand() {
        let cli = Cli::try_parse_from(["linear-sync", "pull", "--env-file", "acme.env"])
            .expect("expected CLI to parse");

        assert_eq!(cli.env_file, Some(PathBuf::from("acme.env")));
        assert!(matches!(cli.command, Commands::Pull(_)));
    }

    #[test]
    fn parses_profile_after_subcommand() {
        let cli = Cli::try_parse_from(["linear-sync", "pull", "--profile", "work"])
            .expect("expected CLI to parse");

        assert_eq!(cli.profile, vec![String::from("work")]);
        assert!(matches!(cli.command, Commands::Pull(_)));
    }

    #[test]
    fn push_force_can_be_explicitly_disabled() {
        let cli = Cli::try_parse_from(["linear-sync", "push", "--no-force"])
            .expect("expected CLI to parse");

        match cli.command {
            Commands::Push(args) => {
                assert_eq!(args.force_override(), Some(ForceSelection::None));
            }
            Commands::Pull(_) => panic!("expected push command"),
        }
    }

    #[test]
    fn pull_bool_overrides_are_tri_state() {
        let cli = Cli::try_parse_from(["linear-sync", "pull", "--no-confirm"])
            .expect("expected CLI to parse");

        match cli.command {
            Commands::Pull(args) => {
                assert_eq!(args.confirm_override(), Some(false));
                assert_eq!(args.include_done_override(), None);
            }
            Commands::Push(_) => panic!("expected pull command"),
        }
    }
}
