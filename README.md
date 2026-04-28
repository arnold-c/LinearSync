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

Fetches active issues from Linear and writes them to Markdown files.

#### Options

- `-t, --team-id <TEAM_ID>`: Pull issues for a specific team
    - You should use the team identifier used in issue-titles e.g., `ACA` for `ACA-001`
- `-o, --output-dir <PATH>`: Output directory for generated Markdown files
- `-m, --merge-all-teams`: Merge issues from all teams into a single directory
- `-c, --confirm`: Interactively confirm team selection, merge behavior, and output path

#### Examples

Pull issues from all teams into the default directory structure:

```bash
cargo run -- pull
```

Pull issues for a specific team:

```bash
cargo run -- pull --team-id <TEAM_ID>
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

### `push`

Starts the push flow for local updates back to Linear.

```bash
cargo run -- push --input-dir /path/to/notes
```

> Note: the current implementation only initializes the push command and does not yet sync changes back to Linear.

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

Each issue is exported as a Markdown file with YAML frontmatter and a description block.

Example:

```md
---
title: "Fix login redirect"
status: "In Progress"
priority: 0
linear_id: "<linear-issue-id>"
tags:
  - bug
  - backend
github_links:
  - "https://github.com/org/repo/pull/123"
project: "[[Authentication]]"
---

>[!info] Description
> Investigate and fix the redirect loop after login.

[Open in Linear](https://linear.app/...)

---
*Last synced: 2026-04-28 12:00:00*
```

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

- `push` is not fully implemented yet
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

