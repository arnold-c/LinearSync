use std::path::Path;

pub(crate) const ANSI_RED: &str = "\x1b[31m";
pub(crate) const ANSI_GREEN: &str = "\x1b[32m";
pub(crate) const ANSI_YELLOW: &str = "\x1b[33m";
pub(crate) const ANSI_BLUE: &str = "\x1b[34m";
pub(crate) const ANSI_RESET: &str = "\x1b[0m";

pub(crate) fn print_colored_diff(diff: &str) {
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

pub(crate) fn print_push_diff(
    use_delta: bool,
    identifier: &str,
    diff_label: &str,
    file_path: &Path,
    diff: &str,
) {
    if use_delta {
        print_delta_output(&format_delta_patch(identifier, diff_label, file_path, diff));
    } else {
        print_colored_diff(diff);
    }
}

pub(crate) fn format_delta_patch(
    identifier: &str,
    team_name: &str,
    file_path: &Path,
    diff: &str,
) -> String {
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

pub(crate) fn print_delta_output(delta_output: &str) {
    if print_diff_with_delta(delta_output).is_none() {
        print_yaml_style_diff_from_delta_output(delta_output);
    }
}

pub(crate) fn print_yaml_style_diff_from_delta_output(delta_output: &str) {
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

pub(crate) fn parse_delta_patch_header(line: &str) -> Option<String> {
    let rest = line.strip_prefix("@@ ")?;
    let header = rest.strip_suffix(" @@")?;
    Some(header.to_string())
}

pub(crate) fn normalize_delta_fallback_line(line: &str) -> String {
    if let Some(rest) = line.strip_prefix('-') {
        format!("- {}", rest)
    } else if let Some(rest) = line.strip_prefix('+') {
        format!("+ {}", rest)
    } else {
        line.trim_start().to_string()
    }
}

pub(crate) fn print_diff_with_delta(diff: &str) -> Option<()> {
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
