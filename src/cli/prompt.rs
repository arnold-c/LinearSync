use crate::error::AppError;
use crate::linear::models::TeamInfo;
use crate::notes::paths::{default_output_dir_for_team, default_output_root_for_all_teams};
use std::io::{self, Write};
use std::path::PathBuf;

const ALL_TEAMS_OPTION: &str = "ALL TEAMS";

pub(crate) enum PullSelection {
    SingleTeam {
        team: TeamInfo,
        output_dir: PathBuf,
    },
    AllTeams {
        root_output_dir: PathBuf,
        merge_all_teams: bool,
    },
}

pub(crate) fn resolve_pull_selection(
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

pub(crate) fn prompt_for_pull_selection(
    teams: &[TeamInfo],
    team_id: Option<String>,
    output_dir: Option<PathBuf>,
    merge_all_teams: bool,
) -> Result<PullSelection, AppError> {
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
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

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
        let confirmed_merge_all_teams = prompt_for_merge_all_teams(merge_all_teams)?;
        let root_dir = output_dir
            .unwrap_or_else(|| default_output_root_for_all_teams(confirmed_merge_all_teams));
        let confirmed_root_dir = prompt_for_output_dir(
            &format!("Output directory for {}", ALL_TEAMS_OPTION),
            root_dir,
        )?;
        Ok(PullSelection::AllTeams {
            root_output_dir: confirmed_root_dir,
            merge_all_teams: confirmed_merge_all_teams,
        })
    } else {
        let team = teams[selected_index - 1].clone();
        let team_dir = output_dir.unwrap_or_else(|| default_output_dir_for_team(&team.name));
        let confirmed_team_dir =
            prompt_for_output_dir(&format!("Output directory for {}", team.name), team_dir)?;
        Ok(PullSelection::SingleTeam {
            team,
            output_dir: confirmed_team_dir,
        })
    }
}

pub(crate) fn prompt_for_merge_all_teams(default_value: bool) -> Result<bool, AppError> {
    let default_label = if default_value { "Y/n" } else { "y/N" };
    loop {
        print!(
            "Merge all teams into a single subdirectory? [{}]: ",
            default_label
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let trimmed = input.trim().to_lowercase();
        match trimmed.as_str() {
            "" => return Ok(default_value),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Please enter y, yes, n, no, or press Enter to accept the default."),
        }
    }
}

pub(crate) fn prompt_for_output_dir(
    label: &str,
    default_dir: PathBuf,
) -> Result<PathBuf, AppError> {
    println!("{} [{}]", label, default_dir.display());
    print!("Press Enter to accept or type a different path: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default_dir)
    } else {
        Ok(PathBuf::from(trimmed))
    }
}
