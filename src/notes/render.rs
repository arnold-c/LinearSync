use crate::linear::models::{PriorityInfo, RemoteIssue, get_priority_label};
use crate::notes::frontmatter::{extract_ignored_properties, override_frontmatter_value};
use crate::notes::sections::{MANAGED_SECTION_END, MANAGED_SECTION_START, ensure_managed_section};
use serde_yaml::Value as YamlValue;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::OnceLock;

pub(crate) const DEFAULT_TEMPLATE_PATH: &str = "template.md";

pub(crate) static INSTALLED_TEMPLATE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

pub(crate) struct TemplateContext<'a> {
    pub(crate) title: &'a str,
    pub(crate) status: &'a str,
    pub(crate) priority: &'a str,
    pub(crate) issue_id: &'a str,
    pub(crate) identifier: &'a str,
    pub(crate) url: &'a str,
    pub(crate) project: &'a str,
    pub(crate) description_section: &'a str,
    pub(crate) labels_yaml: &'a str,
    pub(crate) gh_yaml: &'a str,
    pub(crate) now: &'a str,
    pub(crate) team_name: &'a str,
}

pub(crate) struct RenderedNote {
    pub(crate) content: String,
    pub(crate) ignored_properties: Vec<String>,
}

pub(crate) fn load_template(template_path: Option<&Path>) -> Option<String> {
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

pub(crate) fn default_template_path_if_present() -> Option<PathBuf> {
    default_template_search_paths()
        .into_iter()
        .find(|path| path.is_file())
}

pub(crate) fn default_template_search_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(DEFAULT_TEMPLATE_PATH)];

    if let Some(installed_path) = installed_template_path() {
        paths.push(installed_path.clone());
    }

    paths
}

pub(crate) fn initialize_installed_template_path() {
    let _ = INSTALLED_TEMPLATE_PATH.get_or_init(|| {
        env::current_exe().ok().map(|exe_path| {
            exe_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(DEFAULT_TEMPLATE_PATH)
        })
    });
}

pub(crate) fn installed_template_path() -> Option<&'static PathBuf> {
    INSTALLED_TEMPLATE_PATH.get().and_then(|path| path.as_ref())
}

pub(crate) fn render_template(template: &str, context: &TemplateContext<'_>) -> RenderedNote {
    let rendered = template
        .replace("{{title}}", context.title)
        .replace("{{status}}", context.status)
        .replace("{{priority}}", context.priority)
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

pub(crate) fn description_section_from_text(description: &str) -> String {
    let description = description.trim();
    if description.is_empty() {
        String::new()
    } else {
        let formatted_description = description.replace("\n", "\n> ");
        format!(">[!info]+ Description\n> {formatted_description}\n\n")
    }
}

pub(crate) fn labels_yaml_from_names(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }

    let mut labels_yaml = String::from("tags:\n");
    for name in names {
        labels_yaml.push_str(&format!("  - {}\n", name.replace(' ', "-")));
    }
    labels_yaml
}

pub(crate) fn github_links_yaml_from_urls(urls: &[String]) -> String {
    let github_urls = urls
        .iter()
        .filter(|url| {
            url.contains("github.com") && (url.contains("/pull/") || url.contains("/issues/"))
        })
        .collect::<Vec<_>>();

    if github_urls.is_empty() {
        return String::new();
    }

    let mut gh_yaml = String::from("github_links:\n");
    for url in github_urls {
        gh_yaml.push_str(&format!("  - \"{}\"\n", url));
    }
    gh_yaml
}

pub(crate) fn render_remote_issue_note(
    issue: &RemoteIssue,
    template: Option<&str>,
    now: &str,
    priority_values: &[PriorityInfo],
) -> RenderedNote {
    let description_section = description_section_from_text(&issue.description);
    let labels_yaml = labels_yaml_from_names(
        &issue
            .labels
            .iter()
            .map(|label| label.name.clone())
            .collect::<Vec<_>>(),
    );
    let gh_yaml = github_links_yaml_from_urls(&issue.attachments);
    let project_name = issue
        .project
        .as_ref()
        .map(|project| project.name.as_str())
        .unwrap_or("");

    let mut rendered = match template {
        Some(template) => render_template(
            template,
            &TemplateContext {
                title: &issue.title,
                status: &issue.status,
                priority: get_priority_label(priority_values, issue.priority),
                issue_id: &issue.id,
                identifier: &issue.identifier,
                url: &issue.url,
                project: project_name,
                description_section: &description_section,
                labels_yaml: &labels_yaml,
                gh_yaml: &gh_yaml,
                now,
                team_name: &issue.team.name,
            },
        ),
        None => default_markdown_content(
            &issue.title,
            &issue.status,
            get_priority_label(priority_values, issue.priority),
            &issue.id,
            &labels_yaml,
            &gh_yaml,
            project_name,
            &description_section,
            &issue.url,
            now,
        ),
    };

    rendered.content = override_frontmatter_value(
        &rendered.content,
        "priority",
        YamlValue::String(get_priority_label(priority_values, issue.priority).to_string()),
    );
    rendered
}

pub(crate) fn default_markdown_content(
    title: &str,
    status: &str,
    priority: &str,
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
priority: "{priority}"
linear_id: "[{issue_id}]({url})"
{labels_yaml}{gh_yaml}project: "[[{project}]]"
---

{MANAGED_SECTION_START}
{description_section}---
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
