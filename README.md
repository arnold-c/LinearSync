# LinearSync

LinearSync is a small Rust CLI for syncing active Linear issues into local Markdown files.
It is designed for note-taking workflows such as Obsidian, where each issue becomes a Markdown note with frontmatter, description, links, and metadata.

## Features

- Pull active issues from Linear via the GraphQL API
- Export issues as Markdown files
- Organize files by team and status
- Optionally merge issues from all teams into one directory
- Interactive pull confirmation flow for team and output selection
- Extract GitHub issue and PR links from Linear attachments
- Load the Linear API key from environment variables or a `.env` file

## Requirements

- Rust and Cargo
- A Linear API key

## Installation

Clone the repository and build the project with Cargo:

```bash
cargo build --release
```

The compiled binary will be available at:

```text
./target/release/LinearSync
```

Depending on your Cargo configuration, you may also want to rename or install it as `linear-sync`.

I personally install the tool so it is globally accessible.

```bash
cd /path/to/LinearSync/clone
cargo install --path .
```

## Configuration

LinearSync requires a Linear API key.

You can provide it in either of these ways:

### Option 1: `.env` file

Create a `.env` file in the directory where you run the command:

```env
LINEAR_API_KEY=lin_api_your_key_here
```

### Option 2: shell environment

```bash
export LINEAR_API_KEY=lin_api_your_key_here
```

## Usage

```bash
cargo run -- pull [OPTIONS]
cargo run -- push --input-dir <PATH>
```

Or with the built binary:

```bash
./target/release/LinearSync pull [OPTIONS]
./target/release/LinearSync push --input-dir <PATH>
```

## Commands

### `pull`

Fetches issues from Linear and writes them to Markdown files.
By default, issues already in `Done` are skipped unless they need to be moved from a non-`done` location.

#### Options

- `-t, --team-id <TEAM_ID>`: Pull issues for a specific team
    - You should use the team identifier used in issue-titles e.g., `ACA` for `ACA-001`
- `-o, --output-dir <PATH>`: Output directory for generated Markdown files
- `--issue-id <ISSUE-ID>`: Pull only a single issue, such as `ACA-125`
- `--template <PATH>`: Use a specific Markdown template file for created notes
- `-m, --merge-all-teams`: Merge issues from all teams into a single directory
- `-c, --confirm`: Interactively confirm team selection, merge behavior, and output path
- `--force`: Overwrite the entire note instead of only updating the managed section
- `--include-done`: Include issues already in `done/` or not yet created locally with a `Done` status
- `--dry-run`: Preview note changes without writing files

#### Examples

Pull issues from all teams into the default directory structure:

```bash
cargo run -- pull
```

Pull issues for a specific team:

```bash
cargo run -- pull --team-id <TEAM_ID>
```

Pull a single issue:

```bash
cargo run -- pull --issue-id ACA-125
```

Pull issues from all teams into one shared directory:

```bash
cargo run -- pull --merge-all-teams
```

Run the interactive selection flow:

```bash
cargo run -- pull --confirm
```

Write output to a custom directory:

```bash
cargo run -- pull --output-dir /path/to/obsidian/linear
```

Use a specific template file:

```bash
cargo run -- pull --template ./template.md
```

Force a full overwrite of existing notes:

```bash
cargo run -- pull --force
```

Preview a pull without writing files:

```bash
cargo run -- pull --dry-run
```

Include all Done issues too:

```bash
cargo run -- pull --include-done
```

### `push`

Checks local notes against Linear and reports differences.
By default it does **not** push frontmatter changes automatically.
Instead, it prints diffs to stdout and writes a warning block into the local note.

```bash
cargo run -- push --input-dir /path/to/notes
```

Push a single issue:

```bash
cargo run -- push --input-dir /path/to/notes --issue-id ACA-125
```

#### Options

- `-i, --input-dir <PATH>`: Root directory containing issue notes
- `--issue-id <ISSUE-ID>`: Push only a single issue, such as `ACA-125`
- `-p, --template <PATH>`: Use a specific template when diffing the managed block
- `--force[=<PROPERTY,...>]`: Push supported frontmatter properties to Linear
    - supported properties: `title`, `status`, `priority`, `project`, `tags`
    - `--force` updates all supported differing properties found in the local note
    - `--force=title,status` updates only the listed properties
- `--include-done`: Include notes already under a `done/` subdirectory
- `--dry-run`: Preview changes without updating Linear or editing local notes
- `--no-delta`: Use YAML-style diff output instead of delta rendering

#### Push behavior

- The file name stem is the primary issue identifier, with `linear_id` used as a fallback
    - This assumes that the note's file name has not been changed from the created on based on the Linear issue number
- Frontmatter differences are reported unless they are listed in `ignored_properties`
- Content outside the managed block is ignored by `push`
- Managed block edits are reported, but never pushed; edit the issue in Linear instead
- By default, `push` skips notes already under `done/`; use `--include-done` to scan them
- If a forced status update moves an issue to `Done`, the note is moved to the `done/` subdirectory
- By default, `pull` skips issues already `Done` when they already live in `done/` or do not yet exist locally
- Issues transitioning between active statuses and `Done` are always processed so the corresponding note is updated or moved
- With `--issue-id`, `pull` and `push` only act on the matching issue while preserving the same location mismatch warnings and move/update guidance

## Output Structure

By default, pulled issues are written under:

```text
linear-issues/
```

### Single team

```text
linear-issues/<team-slug>/<status>/<ISSUE-ID>.md
```

Example:

```text
linear-issues/platform/in-progress/ENG-123.md
```

### All teams, separate directories

```text
linear-issues/<team-slug>/<status>/<ISSUE-ID>.md
```

### All teams, merged

```text
linear-issues/all-teams/<status>/<ISSUE-ID>.md
```

Team names are slugified to lowercase and non-alphanumeric characters are replaced with hyphens.

## Markdown Format

Each issue is exported as a Markdown file with a managed section.
By default, `pull` only updates that managed section and leaves the rest of the note untouched, so you can add your own notes outside it.
Use `--force` to overwrite the full file.

If Linear reports a new status but the matching note is found in a different status subdirectory,
`pull` updates the note in place and inserts a warning telling you where to move it in Obsidian.
This avoids breaking backlinks by renaming or moving the file automatically.

### Templates

`pull` and `push` resolve templates in this order:

1. the path passed via `--template`
2. `./template.md` in your current working directory
3. `template.md` next to the installed binary

This repository includes a default `template.md` at the project root.
If you run from the repo, it will be picked up automatically.

If you install with `cargo install --path .`, Cargo installs the binary but does
not automatically copy `template.md` alongside it. That means the installed-binary
fallback only works if you manually place a `template.md` next to the binary.
On most systems that binary is in `~/.cargo/bin/`, so the fallback path would
typically be:

```text
~/.cargo/bin/template.md
```

Supported placeholders:

- `{{title}}`
- `{{status}}`
- `{{linear_id}}`
- `{{identifier}}`
- `{{url}}`
- `{{project}}`
- `{{description_section}}`
    - Renders the complete Description callout when a Linear issue has a description, otherwise renders nothing
- `{{labels_yaml}}`
- `{{github_links_yaml}}`
- `{{last_synced}}`
- `{{team_name}}`

Example:

```md
---
title: "{{title}}"
status: "{{status}}"
priority: 0
linear_id: "[{{linear_id}}]({{url}})"
ignored_properties: aliases, id
{{labels_yaml}}{{github_links_yaml}}project: "[[{{project}}]]"
---

<!-- linear-sync:managed:start -->
{{description_section}}[Open in Linear]({{url}})

---
*Last synced: {{last_synced}}*

## My notes
<!-- linear-sync:managed:end -->
```

When an issue has no description, `{{description_section}}` renders as an empty string and the note omits the Description callout.

Anything outside the managed section is preserved by future `pull` runs and ignored by `push`.

## What gets synced

Currently, the `pull` command exports:

- issue ID
- issue identifier
- title
- URL
- description
- workflow state
- labels
- project name
- GitHub issue and PR attachment links

The current issue filter includes active work with Linear state types:

- `started`
- `unstarted`

## Dependencies

This project uses:

- `reqwest` for HTTP requests
- `serde` and `serde_json` for JSON handling
- `clap` for CLI parsing
- `chrono` for timestamps
- `dotenvy` for `.env` support

## Limitations

- `push` only updates supported frontmatter-backed Linear fields when forced
- Managed block changes are never pushed back to Linear
- Error handling is mostly CLI-oriented and exits on API failures
- Output format is opinionated toward Markdown note workflows

## Development

Run in development mode:

```bash
cargo run -- pull --confirm
```

Build a release binary:

```bash
cargo build --release
```

