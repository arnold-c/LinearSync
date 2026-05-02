#[derive(Clone, Debug)]
pub(crate) struct TeamInfo {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PriorityInfo {
    pub(crate) priority: i64,
    pub(crate) label: String,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkflowState {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct LabelInfo {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectInfo {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RemoteIssue {
    pub(crate) id: String,
    pub(crate) identifier: String,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) description: String,
    pub(crate) updated_at: String,
    pub(crate) status: String,
    pub(crate) priority: i64,
    pub(crate) team: TeamInfo,
    pub(crate) states: Vec<WorkflowState>,
    pub(crate) labels: Vec<LabelInfo>,
    pub(crate) available_labels: Vec<LabelInfo>,
    pub(crate) project: Option<ProjectInfo>,
    pub(crate) attachments: Vec<String>,
}

pub(crate) fn get_priority_label<'a>(
    priority_values: &'a [PriorityInfo],
    priority: i64,
) -> &'a str {
    priority_values
        .iter()
        .find(|v| v.priority == priority)
        .map(|v| v.label.as_str())
        .unwrap_or("No priority")
}

pub(crate) fn get_priority_number(priority_values: &[PriorityInfo], label: &str) -> Option<i64> {
    priority_values
        .iter()
        .find(|v| v.label.eq_ignore_ascii_case(label))
        .map(|v| v.priority)
        .or_else(|| label.parse::<i64>().ok())
}
