use crate::cli::{
    CacheCommands, CacheRebuildArgs, Cli, Commands, ForceSelection, PullArgs, PushArgs,
    parse_force_selection,
};
use crate::error::AppError;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AppConfig {
    #[serde(default)]
    profiles: BTreeMap<String, ProfileConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct ProfileConfig {
    env_file: Option<PathBuf>,
    template: Option<PathBuf>,
    pull: Option<PullProfileConfig>,
    push: Option<PushProfileConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct PullProfileConfig {
    team_id: Option<String>,
    output_dir: Option<PathBuf>,
    template: Option<PathBuf>,
    merge_all_teams: Option<bool>,
    confirm: Option<bool>,
    force: Option<bool>,
    include_done: Option<bool>,
    dry_run: Option<bool>,
    delta: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
struct PushProfileConfig {
    input_dir: Option<PathBuf>,
    template: Option<PathBuf>,
    force: Option<PushForceConfig>,
    include_done: Option<bool>,
    dry_run: Option<bool>,
    delta: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum PushForceConfig {
    Boolean(bool),
    String(String),
    List(Vec<String>),
}

pub(crate) struct LoadedConfig {
    path: PathBuf,
    config: AppConfig,
}

#[derive(Debug)]
pub(crate) struct ExecutionPlan {
    pub(crate) profile_name: Option<String>,
    pub(crate) env_file: Option<PathBuf>,
    pub(crate) command: EffectiveCommand,
}

#[derive(Debug)]
pub(crate) enum EffectiveCommand {
    Pull(EffectivePullArgs),
    Push(EffectivePushArgs),
    CacheRebuild(EffectiveCacheRebuildArgs),
}

#[derive(Debug)]
pub(crate) struct EffectivePullArgs {
    pub(crate) team_id: Option<String>,
    pub(crate) issue_id: Option<String>,
    pub(crate) output_dir: Option<PathBuf>,
    pub(crate) template: Option<PathBuf>,
    pub(crate) merge_all_teams: bool,
    pub(crate) confirm: bool,
    pub(crate) force: bool,
    pub(crate) include_done: bool,
    pub(crate) dry_run: bool,
    pub(crate) use_delta: bool,
}

#[derive(Debug)]
pub(crate) struct EffectivePushArgs {
    pub(crate) input_dir: PathBuf,
    pub(crate) issue_id: Option<String>,
    pub(crate) template: Option<PathBuf>,
    pub(crate) force_selection: ForceSelection,
    pub(crate) include_done: bool,
    pub(crate) dry_run: bool,
    pub(crate) use_delta: bool,
}

#[derive(Debug)]
pub(crate) struct EffectiveCacheRebuildArgs {
    pub(crate) input_dir: PathBuf,
}

pub(crate) fn load_config(config_path: Option<&Path>) -> Result<Option<LoadedConfig>, AppError> {
    let Some(path) = resolve_config_path(config_path)? else {
        return Ok(None);
    };

    let content = fs::read_to_string(&path).map_err(|error| {
        AppError::message(format!(
            "Failed to read config file `{}`: {error}",
            path.display()
        ))
    })?;

    let config = toml::from_str(&content).map_err(|error| {
        AppError::message(format!(
            "Failed to parse config file `{}`: {error}",
            path.display()
        ))
    })?;

    Ok(Some(LoadedConfig { path, config }))
}

pub(crate) fn build_execution_plans(
    cli: &Cli,
    loaded_config: Option<&LoadedConfig>,
) -> Result<Vec<ExecutionPlan>, AppError> {
    let selected_profiles = select_profiles(cli.profile.as_slice(), loaded_config)?;

    match selected_profiles {
        Some(profile_names) => profile_names
            .into_iter()
            .map(|profile_name| build_profile_plan(cli, loaded_config.unwrap(), &profile_name))
            .collect(),
        None => Ok(vec![build_direct_plan(cli)?]),
    }
}

fn resolve_config_path(config_path: Option<&Path>) -> Result<Option<PathBuf>, AppError> {
    if let Some(path) = config_path {
        let path = normalize_path(path);
        if !path.exists() {
            return Err(AppError::message(format!(
                "Config file `{}` does not exist.",
                path.display()
            )));
        }
        return Ok(Some(path));
    }

    let Some(home_config_dir) = home_config_dir() else {
        return Ok(None);
    };

    let default_path = home_config_dir.join("linear-sync").join("config.toml");
    if default_path.exists() {
        Ok(Some(default_path))
    } else {
        Ok(None)
    }
}

fn select_profiles(
    requested_profiles: &[String],
    loaded_config: Option<&LoadedConfig>,
) -> Result<Option<Vec<String>>, AppError> {
    let Some(config) = loaded_config else {
        if requested_profiles.is_empty() {
            return Ok(None);
        }

        return Err(AppError::message(
            "`--profile` requires a config file. Pass `--config <PATH>` or create ~/.config/linear-sync/config.toml.",
        ));
    };

    if config.config.profiles.is_empty() {
        return Err(AppError::message(format!(
            "Config file `{}` does not define any profiles.",
            config.path.display()
        )));
    }

    if requested_profiles.is_empty()
        || requested_profiles
            .iter()
            .any(|profile| profile.eq_ignore_ascii_case("all"))
    {
        return Ok(Some(config.config.profiles.keys().cloned().collect()));
    }

    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();

    for profile_name in requested_profiles {
        if !config.config.profiles.contains_key(profile_name) {
            return Err(AppError::message(format!(
                "Profile `{profile_name}` was not found in `{}`.",
                config.path.display()
            )));
        }

        if seen.insert(profile_name.clone()) {
            selected.push(profile_name.clone());
        }
    }

    Ok(Some(selected))
}

fn build_direct_plan(cli: &Cli) -> Result<ExecutionPlan, AppError> {
    let command = match &cli.command {
        Commands::Pull(args) => EffectiveCommand::Pull(resolve_pull_args(args, None, None)),
        Commands::Push(args) => {
            EffectiveCommand::Push(resolve_push_args(args, None, None).map_err(AppError::message)?)
        }
        Commands::Cache(args) => match &args.command {
            CacheCommands::Rebuild(rebuild) => {
                EffectiveCommand::CacheRebuild(resolve_cache_rebuild_args(rebuild))
            }
        },
    };

    Ok(ExecutionPlan {
        profile_name: None,
        env_file: cli.env_file.as_deref().map(normalize_path),
        command,
    })
}

fn build_profile_plan(
    cli: &Cli,
    loaded_config: &LoadedConfig,
    profile_name: &str,
) -> Result<ExecutionPlan, AppError> {
    let profile = loaded_config
        .config
        .profiles
        .get(profile_name)
        .expect("selected profiles must exist");
    let config_dir = loaded_config.config_dir();

    let env_file = cli.env_file.as_deref().map(normalize_path).or_else(|| {
        profile
            .env_file
            .as_deref()
            .map(|path| resolve_profile_path(path, config_dir))
    });

    let command = match &cli.command {
        Commands::Pull(args) => EffectiveCommand::Pull(resolve_pull_args(
            args,
            profile.pull.as_ref(),
            Some(ProfileContext {
                config_dir,
                shared_template: profile.template.as_deref(),
            }),
        )),
        Commands::Push(args) => EffectiveCommand::Push(
            resolve_push_args(
                args,
                profile.push.as_ref(),
                Some(ProfileContext {
                    config_dir,
                    shared_template: profile.template.as_deref(),
                }),
            )
            .map_err(|message| AppError::message(format!("Profile `{profile_name}`: {message}")))?,
        ),
        Commands::Cache(args) => match &args.command {
            CacheCommands::Rebuild(rebuild) => {
                EffectiveCommand::CacheRebuild(resolve_cache_rebuild_args(rebuild))
            }
        },
    };

    Ok(ExecutionPlan {
        profile_name: Some(profile_name.to_string()),
        env_file,
        command,
    })
}

#[derive(Clone, Copy)]
struct ProfileContext<'a> {
    config_dir: &'a Path,
    shared_template: Option<&'a Path>,
}

fn resolve_pull_args(
    args: &PullArgs,
    profile: Option<&PullProfileConfig>,
    context: Option<ProfileContext<'_>>,
) -> EffectivePullArgs {
    let profile_template = profile.and_then(|profile| profile.template.as_deref());

    EffectivePullArgs {
        team_id: args
            .team_id
            .clone()
            .or_else(|| profile.and_then(|profile| profile.team_id.clone())),
        issue_id: args.issue_id.clone(),
        output_dir: args.output_dir.as_deref().map(normalize_path).or_else(|| {
            profile.and_then(|profile| {
                profile
                    .output_dir
                    .as_deref()
                    .zip(context)
                    .map(|(path, context)| resolve_profile_path(path, context.config_dir))
            })
        }),
        template: resolve_template_path(args.template.as_deref(), profile_template, context),
        merge_all_teams: merge_bool(
            args.merge_all_teams_override(),
            profile.and_then(|profile| profile.merge_all_teams),
            false,
        ),
        confirm: merge_bool(
            args.confirm_override(),
            profile.and_then(|profile| profile.confirm),
            false,
        ),
        force: merge_bool(
            args.force_override(),
            profile.and_then(|profile| profile.force),
            false,
        ),
        include_done: merge_bool(
            args.include_done_override(),
            profile.and_then(|profile| profile.include_done),
            false,
        ),
        dry_run: merge_bool(
            args.dry_run_override(),
            profile.and_then(|profile| profile.dry_run),
            false,
        ),
        use_delta: merge_bool(
            args.delta_override(),
            profile.and_then(|profile| profile.delta),
            true,
        ),
    }
}

fn resolve_push_args(
    args: &PushArgs,
    profile: Option<&PushProfileConfig>,
    context: Option<ProfileContext<'_>>,
) -> Result<EffectivePushArgs, String> {
    let profile_template = profile.and_then(|profile| profile.template.as_deref());
    let input_dir = args.input_dir.as_deref().map(normalize_path).or_else(|| {
        profile.and_then(|profile| {
            profile
                .input_dir
                .as_deref()
                .zip(context)
                .map(|(path, context)| resolve_profile_path(path, context.config_dir))
        })
    });

    let Some(input_dir) = input_dir else {
        return Err(String::from(
            "`push` requires `input_dir` in the selected profile or `--input-dir` on the command line.",
        ));
    };

    Ok(EffectivePushArgs {
        input_dir,
        issue_id: args.issue_id.clone(),
        template: resolve_template_path(args.template.as_deref(), profile_template, context),
        force_selection: resolve_push_force_selection(args, profile),
        include_done: merge_bool(
            args.include_done_override(),
            profile.and_then(|profile| profile.include_done),
            false,
        ),
        dry_run: merge_bool(
            args.dry_run_override(),
            profile.and_then(|profile| profile.dry_run),
            false,
        ),
        use_delta: merge_bool(
            args.delta_override(),
            profile.and_then(|profile| profile.delta),
            true,
        ),
    })
}

fn resolve_cache_rebuild_args(args: &CacheRebuildArgs) -> EffectiveCacheRebuildArgs {
    EffectiveCacheRebuildArgs {
        input_dir: normalize_path(&args.input_dir),
    }
}

fn resolve_push_force_selection(
    args: &PushArgs,
    profile: Option<&PushProfileConfig>,
) -> ForceSelection {
    match args.force_override() {
        Some(selection) => selection,
        None => profile
            .and_then(|profile| profile.force.as_ref())
            .map(force_selection_from_config)
            .unwrap_or(ForceSelection::None),
    }
}

fn force_selection_from_config(force: &PushForceConfig) -> ForceSelection {
    match force {
        PushForceConfig::Boolean(true) => ForceSelection::All,
        PushForceConfig::Boolean(false) => ForceSelection::None,
        PushForceConfig::String(value) => parse_force_selection(std::slice::from_ref(value)),
        PushForceConfig::List(values) => parse_force_selection(values),
    }
}

fn resolve_template_path(
    cli_template: Option<&Path>,
    profile_template: Option<&Path>,
    context: Option<ProfileContext<'_>>,
) -> Option<PathBuf> {
    cli_template.map(normalize_path).or_else(|| {
        profile_template
            .zip(context)
            .map(|(path, context)| resolve_profile_path(path, context.config_dir))
            .or_else(|| {
                context.and_then(|context| {
                    context
                        .shared_template
                        .map(|path| resolve_profile_path(path, context.config_dir))
                })
            })
    })
}

fn merge_bool(cli_override: Option<bool>, profile_value: Option<bool>, default: bool) -> bool {
    cli_override.or(profile_value).unwrap_or(default)
}

fn resolve_profile_path(path: &Path, config_dir: &Path) -> PathBuf {
    let normalized = normalize_path(path);
    if normalized.is_absolute() {
        normalized
    } else {
        config_dir.join(normalized)
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if path_str == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }

    if let Some(suffix) = path_str.strip_prefix("~/")
        && let Some(home_dir) = home_dir()
    {
        return home_dir.join(suffix);
    }

    path.to_path_buf()
}

fn home_config_dir() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

impl LoadedConfig {
    fn config_dir(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn parse_cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("expected CLI to parse")
    }

    fn load_config_from_str(content: &str, path: &str) -> LoadedConfig {
        LoadedConfig {
            path: PathBuf::from(path),
            config: toml::from_str(content).expect("expected config to parse"),
        }
    }

    #[test]
    fn no_profile_and_no_config_uses_direct_execution() {
        let cli = parse_cli(&["linear-sync", "pull"]);
        let plans = build_execution_plans(&cli, None).expect("expected execution plan");

        assert_eq!(plans.len(), 1);
        assert!(plans[0].profile_name.is_none());
    }

    #[test]
    fn no_profile_uses_all_configured_profiles() {
        let cli = parse_cli(&["linear-sync", "pull"]);
        let config = load_config_from_str(
            r#"
                [profiles.work]
                env_file = "work.env"

                [profiles.personal]
                env_file = "personal.env"
            "#,
            "/tmp/config.toml",
        );

        let plans = build_execution_plans(&cli, Some(&config)).expect("expected execution plan");

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].profile_name.as_deref(), Some("personal"));
        assert_eq!(plans[1].profile_name.as_deref(), Some("work"));
    }

    #[test]
    fn profile_all_selects_all_profiles() {
        let cli = parse_cli(&["linear-sync", "--profile", "all", "pull"]);
        let config = load_config_from_str(
            r#"
                [profiles.work]
                env_file = "work.env"

                [profiles.personal]
                env_file = "personal.env"
            "#,
            "/tmp/config.toml",
        );

        let plans = build_execution_plans(&cli, Some(&config)).expect("expected execution plan");
        assert_eq!(plans.len(), 2);
    }

    #[test]
    fn cli_overrides_profile_values() {
        let cli = parse_cli(&[
            "linear-sync",
            "--profile",
            "work",
            "pull",
            "--output-dir",
            "./override",
            "--no-confirm",
        ]);
        let config = load_config_from_str(
            r#"
                [profiles.work]
                template = "template.md"

                [profiles.work.pull]
                output_dir = "notes"
                confirm = true
            "#,
            "/tmp/config.toml",
        );

        let plans = build_execution_plans(&cli, Some(&config)).expect("expected execution plan");

        match &plans[0].command {
            EffectiveCommand::Pull(args) => {
                assert_eq!(args.output_dir, Some(PathBuf::from("./override")));
                assert!(!args.confirm);
                assert_eq!(args.template, Some(PathBuf::from("/tmp/template.md")));
            }
            EffectiveCommand::Push(_) | EffectiveCommand::CacheRebuild(_) => {
                panic!("expected pull plan")
            }
        }
    }

    #[test]
    fn push_requires_input_dir_after_profile_merge() {
        let cli = parse_cli(&["linear-sync", "--profile", "work", "push"]);
        let config = load_config_from_str(
            r#"
                [profiles.work]
                env_file = "work.env"
            "#,
            "/tmp/config.toml",
        );

        let error = build_execution_plans(&cli, Some(&config)).expect_err("expected error");
        assert!(
            error
                .to_string()
                .contains("requires `input_dir` in the selected profile")
        );
    }

    #[test]
    fn push_force_can_be_loaded_from_config() {
        let cli = parse_cli(&["linear-sync", "--profile", "work", "push"]);
        let config = load_config_from_str(
            r#"
                [profiles.work.push]
                input_dir = "notes"
                force = ["title", "status"]
            "#,
            "/tmp/config.toml",
        );

        let plans = build_execution_plans(&cli, Some(&config)).expect("expected execution plan");

        match &plans[0].command {
            EffectiveCommand::Push(args) => match &args.force_selection {
                ForceSelection::Selected(values) => {
                    assert!(values.contains("title"));
                    assert!(values.contains("status"));
                }
                _ => panic!("expected selected force properties"),
            },
            EffectiveCommand::Pull(_) | EffectiveCommand::CacheRebuild(_) => {
                panic!("expected push plan")
            }
        }
    }
}
