use crate::error::AppError;
use crate::linear::models::{
    LabelInfo, PriorityInfo, ProjectInfo, RemoteIssue, TeamInfo, WorkflowState,
};
use crate::notes::discovery::LocalNote;
use crate::notes::paths::status_slug;
use reqwest::blocking::Client;
use serde_json::{Map as JsonMap, Value, json};

pub(crate) const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

pub(crate) fn fetch_teams(client: &Client, api_key: &str) -> Result<Vec<TeamInfo>, AppError> {
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

    let response = graphql_request(client, api_key, query, json!({}))?;
    let teams = response["data"]["teams"]["nodes"]
        .as_array()
        .ok_or_else(|| AppError::message("Could not retrieve team list from Linear."))?;

    Ok(teams
        .iter()
        .filter_map(|team| {
            Some(TeamInfo {
                id: team["id"].as_str()?.to_string(),
                name: team["name"].as_str()?.to_string(),
            })
        })
        .collect())
}

pub(crate) fn fetch_priority_values(
    client: &Client,
    api_key: &str,
) -> Result<Vec<PriorityInfo>, AppError> {
    let query = r#"
    query GetPriorityValues {
      issuePriorityValues {
        priority
        label
      }
    }
    "#;

    let response = graphql_request(client, api_key, query, json!({}))?;
    let values = response["data"]["issuePriorityValues"]
        .as_array()
        .ok_or_else(|| {
            AppError::message("Could not retrieve issue priority values from Linear.")
        })?;

    Ok(values
        .iter()
        .filter_map(|val| {
            Some(PriorityInfo {
                priority: val["priority"].as_i64()?,
                label: val["label"].as_str()?.to_string(),
            })
        })
        .collect())
}

pub(crate) fn fetch_remote_issue_for_note(
    client: &Client,
    api_key: &str,
    local_note: &LocalNote,
) -> Result<Option<RemoteIssue>, AppError> {
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

pub(crate) fn fetch_remote_issue_by_identifier(
    client: &Client,
    api_key: &str,
    identifier: &str,
) -> Result<Option<RemoteIssue>, AppError> {
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

pub(crate) fn fetch_remote_issue_by_issue_v2_identifier(
    client: &Client,
    api_key: &str,
    identifier: &str,
) -> Result<Option<RemoteIssue>, AppError> {
    let query = r#"
    query GetIssueByIdentifier($identifier: String!) {
      issueV2(identifier: $identifier) {
        id
        identifier
        title
        url
        description
        updatedAt
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

    let response = graphql_request(client, api_key, query, json!({ "identifier": identifier }))?;
    let issue = response["data"]["issueV2"]
        .as_object()
        .cloned()
        .map(Value::Object);
    issue.map(parse_remote_issue).transpose()
}

pub(crate) fn fetch_remote_issue_by_team_and_number(
    client: &Client,
    api_key: &str,
    team_key: &str,
    issue_number: i64,
) -> Result<Option<RemoteIssue>, AppError> {
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
            updatedAt
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

    let response = graphql_request(
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

pub(crate) fn fetch_remote_issue_by_id(
    client: &Client,
    api_key: &str,
    issue_id: &str,
) -> Result<Option<RemoteIssue>, AppError> {
    let query = r#"
    query GetIssueById($id: String!) {
      issue(id: $id) {
        id
        identifier
        title
        url
        description
        updatedAt
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

    let response = graphql_request(client, api_key, query, json!({ "id": issue_id }))?;
    let issue = response["data"]["issue"]
        .as_object()
        .cloned()
        .map(Value::Object);
    issue.map(parse_remote_issue).transpose()
}

pub(crate) fn fetch_project_by_name(
    client: &Client,
    api_key: &str,
    project_name: &str,
) -> Result<Option<ProjectInfo>, AppError> {
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

    let response = graphql_request(
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

pub(crate) fn update_linear_issue(
    client: &Client,
    api_key: &str,
    issue_id: &str,
    input: JsonMap<String, Value>,
) -> Result<(), AppError> {
    let query = r#"
    mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {
      issueUpdate(id: $id, input: $input) {
        success
      }
    }
    "#;

    let response = graphql_request(
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
        Err(AppError::message(
            "Linear issue update did not report success.",
        ))
    }
}

pub(crate) fn graphql_request(
    client: &Client,
    api_key: &str,
    query: &str,
    variables: Value,
) -> Result<Value, AppError> {
    let response = client
        .post(LINEAR_API_URL)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .map_err(|error| {
            AppError::message(format!("Failed to send request to Linear API: {error}"))
        })?;

    let status = response.status();
    let body = response.text().map_err(|error| {
        AppError::message(format!("Failed to read Linear API response: {error}"))
    })?;

    if !status.is_success() {
        return Err(AppError::message(format!(
            "Linear API request failed with {status}: {body}"
        )));
    }

    let value = serde_json::from_str::<Value>(&body).map_err(|error| {
        AppError::message(format!("Failed to parse Linear API response JSON: {error}"))
    })?;

    if let Some(errors) = value["errors"].as_array()
        && !errors.is_empty()
    {
        return Err(AppError::message(format!(
            "Linear API returned GraphQL errors: {}",
            value["errors"]
        )));
    }

    Ok(value)
}

pub(crate) fn fetch_required_issue(
    client: &Client,
    api_key: &str,
    identifier: &str,
) -> Result<RemoteIssue, AppError> {
    fetch_remote_issue_by_identifier(client, api_key, identifier)?
        .ok_or_else(|| AppError::message(format!("Could not find Linear issue `{identifier}`.")))
}

pub(crate) fn parse_remote_issue(issue: Value) -> Result<RemoteIssue, AppError> {
    let id = issue["id"]
        .as_str()
        .ok_or_else(|| AppError::message("Linear issue is missing an id"))?
        .to_string();
    let identifier = issue["identifier"]
        .as_str()
        .ok_or_else(|| AppError::message("Linear issue is missing an identifier"))?
        .to_string();
    let title = issue["title"].as_str().unwrap_or("No Title").to_string();
    let url = issue["url"].as_str().unwrap_or("").to_string();
    let description = issue["description"].as_str().unwrap_or("").to_string();
    let updated_at = issue["updatedAt"].as_str().unwrap_or("").to_string();
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
        updated_at,
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

pub(crate) fn parse_issue_identifier(identifier: &str) -> Option<(String, i64)> {
    let (team_key, issue_number) = identifier.split_once('-')?;
    let team_key = team_key.trim();
    let issue_number = issue_number.trim().parse::<i64>().ok()?;

    if team_key.is_empty() {
        None
    } else {
        Some((team_key.to_string(), issue_number))
    }
}

pub(crate) fn is_graphql_shape_error(error: &AppError) -> bool {
    matches!(error, AppError::Message(message)
        if message.contains("GRAPHQL_VALIDATION_FAILED")
            || message.contains("Field \"")
            || message.contains("Cannot query field")
            || message.contains("Unknown argument"))
}

pub(crate) fn resolve_state<'a>(
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

pub(crate) fn resolve_label<'a>(
    labels: &'a [LabelInfo],
    desired_label: &str,
) -> Option<&'a LabelInfo> {
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
