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
- Cache per-note sync baselines in `.linear-sync/cache.json`
- Warn when local notes need `push` or remote issues need `pull`
- Skip unchanged notes and issues during sync decisions
- Use the cache as a note index for targeted and full-root push lookups before scanning directories
- Incrementally pull remotely changed issues after the initial team scan
- Load workspace-specific settings from a TOML config with named profiles
- Run one profile, several named profiles, or all configured profiles in one command
- Read the Linear API key from a profile env file, an explicit env file, a `.env` file, or the shell environment

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

LinearSync supports two ways of running:

1. **profile mode** via a TOML config file
2. **direct mode** via CLI flags plus `--env-file`, `.env`, or shell environment

Profile mode is recommended for syncing multiple Linear workspaces.

### Profile config

Default config path:

```text
~/.config/linear-sync/config.toml
```

This repository also includes a fuller example at `./config.toml`.

Minimal example:

```toml
[profiles.work]
env_file = "env/work.env"
template = "templates/work-template.md"

[profiles.work.pull]
team_id = "ACA"
output_dir = "notes/linear/work"
merge_all_teams = false
confirm = false
force = false
include_done = false
dry_run = false
delta = true

[profiles.work.push]
input_dir = "notes/linear/work"
force = ["title", "status", "priority"]
include_done = false
dry_run = false
delta = true

[profiles.personal]
env_file = "env/personal.env"

[profiles.personal.pull]
output_dir = "notes/linear/personal"
merge_all_teams = true

[profiles.personal.push]
input_dir = "notes/linear/personal"
force = "all"
```

Config notes:

- Profile names are user-defined.
- `env_file` stores the path to an env file, not the API key itself.
- Relative paths in the config are resolved relative to the config file.
- Shared profile keys live under `[profiles.<name>]`.
- Command-specific settings live under `[profiles.<name>.pull]` and `[profiles.<name>.push]`.
- `template` can be defined at the profile level or per-command.
- Boolean config values use positive names such as `confirm = false` and `delta = true`.
- `push.force` accepts:
    - `false` to disable pushing frontmatter changes by default
    - `true` or `"all"` to push all supported properties
    - a list such as `["title", "status"]` to push selected properties
- `push.input_dir` must be provided either in the selected profile or via `--input-dir`.
- `issue_id` is CLI-only and is not read from the config file.
- If a config file exists and you omit `--profile`, LinearSync runs **all configured profiles** for the selected command.
- `--profile all` explicitly does the same thing.

Example env file contents:

```env
LINEAR_API_KEY=lin_api_your_key_here
```

### API key resolution

LinearSync resolves the API key in this order:

1. the env file selected by `--env-file`
2. the selected profile's `env_file`
3. the shell environment variable `LINEAR_API_KEY`
4. `.env` in the current directory or its parents

When `--env-file` or a profile `env_file` is used, LinearSync reads `LINEAR_API_KEY` directly from that file. This keeps multi-profile runs isolated so one profile does not leak credentials into another.

### Direct mode without profiles

You can also run without a profile config.

#### Option 1: explicit env file

```bash
cargo run -- --env-file ~/.config/linear-sync/work.env pull
cargo run -- --env-file ~/.config/linear-sync/work.env push --input-dir /path/to/notes
```

#### Option 2: `.env` file

Create a `.env` file in the directory where you run the command:

```env
LINEAR_API_KEY=lin_api_your_key_here
```

This make sense when you are only syncing a single Linear workspace's issues.

#### Option 3: shell environment

```bash
export LINEAR_API_KEY=lin_api_your_key_here
```

## Usage

```bash
cargo run -- [GLOBAL_OPTIONS] pull [OPTIONS]
cargo run -- [GLOBAL_OPTIONS] push [OPTIONS]
```

Or with the built binary:

```bash
./target/release/LinearSync [GLOBAL_OPTIONS] pull [OPTIONS]
./target/release/LinearSync [GLOBAL_OPTIONS] push [OPTIONS]
```

## Commands

### Global options

- `--config <PATH>`: Load profiles from a specific TOML config file instead of `~/.config/linear-sync/config.toml`
- `--profile <NAME[,NAME...]>`: Run one or more named profiles from the config file
    - use `all`, or omit the flag entirely when a config file exists, to run all configured profiles
- `-e, --env-file <PATH>`: Override the selected profile's `env_file` or read `LINEAR_API_KEY` from a specific env file in direct mode

### `pull`

Fetches issues from Linear and writes them to Markdown files.
By default, issues already in `Done` are skipped unless they need to be moved from a non-`done` location.
After the first successful scan for a team and note root, later pulls query Linear incrementally using cached remote scan timestamps.

#### Options

- `-t, --team-id <TEAM_ID>`: Pull issues for a specific team
    - You should use the team identifier used in issue-titles e.g., `ACA` for `ACA-001`
- `-o, --output-dir <PATH>`: Output directory for generated Markdown files
- `--issue-id <ISSUE-ID>`: Pull only a single issue, such as `ACA-125`
- `--template <PATH>`: Use a specific Markdown template file for created notes
- `-m, --merge-all-teams`: Merge issues from all teams into a single directory
- `--separate-team-dirs`: Keep separate team subdirectories when pulling all teams
- `--confirm | --no-confirm`: Enable or disable the interactive confirmation flow
- `--force | --no-force`: Overwrite the full note or preserve the unmanaged body
- `--include-done | --skip-done`: Include or skip Done issues by default
- `--dry-run | --no-dry-run`: Preview changes or apply them
- `--delta | --no-delta`: Use delta rendering or YAML-style diff output

#### Examples

Pull issues from all teams into the default directory structure:

```bash
cargo run -- pull
```

If a config file exists and no `--profile` is passed, this runs `pull` for all configured profiles.

Use a specific config file:

```bash
cargo run -- --config ./config.toml --profile work pull
```

Pull issues for a specific team:

```bash
cargo run -- pull --team-id <TEAM_ID>
```

Pull a single named profile:

```bash
cargo run -- --profile work pull
```

Pull all configured profiles explicitly:

```bash
cargo run -- --profile all pull
```

Pull using a workspace-specific env file without profiles:

```bash
cargo run -- --env-file ~/.config/linear-sync/work.env pull --team-id <TEAM_ID>
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
It also consults the local sync cache and warns when a `pull` is needed first.

```bash
cargo run -- push --input-dir /path/to/notes
```

Push a single named profile:

```bash
cargo run -- --profile work push
```

Use a specific config file:

```bash
cargo run -- --config ./config.toml --profile work push
```

Push all configured profiles explicitly:

```bash
cargo run -- --profile all push
```

Push using a workspace-specific env file without profiles:

```bash
cargo run -- --env-file ~/.config/linear-sync/work.env push --input-dir /path/to/notes
```

Push a single issue:

```bash
cargo run -- push --input-dir /path/to/notes --issue-id ACA-125
```

#### Options

- `-i, --input-dir <PATH>`: Root directory containing issue notes
- `--issue-id <ISSUE-ID>`: Push only a single issue, such as `ACA-125`
- `-p, --template <PATH>`: Use a specific template when diffing the managed block
- `--force[=<PROPERTY,...>] | --no-force`: Push supported frontmatter properties to Linear or disable configured push updates
    - supported properties: `title`, `status`, `priority`, `project`, `tags`
    - `--force` updates all supported differing properties found in the local note
    - `--force=title,status` updates only the listed properties
- `--include-done | --skip-done`: Include or skip notes already under a `done/` subdirectory
- `--dry-run | --no-dry-run`: Preview changes or apply them
- `--delta | --no-delta`: Use delta rendering or YAML-style diff output

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
- `push` warns and skips forced updates when Linear changed since the last successful sync and a `pull` is needed first
- `pull` warns and skips overwriting notes when local pushable metadata changed since the last successful sync and a `push` is needed first
- `pull` and `push` both warn when both sides changed since the last successful sync
- With `--issue-id`, `pull` and `push` only act on the matching issue while preserving the same location mismatch warnings and move/update guidance
- For targeted issue lookups, `pull` and `push` consult the cache first and fall back to directory scanning when the cached path is stale or missing
- Full-root `push` reuses a cached local note index when it is fresh, and rebuilds that index from disk when local note directories changed

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

### Sync cache

Each note root also gets a local cache file:

```text
<note-root>/.linear-sync/cache.json
```

Examples:

```text
linear-issues/platform/.linear-sync/cache.json
linear-issues/all-teams/.linear-sync/cache.json
```

The cache stores per-issue sync baselines such as the note path, last synced
Linear `updatedAt`, last synced local push hash, and last sync time. It also
stores a local note index for push discovery and per-team remote scan timestamps
used for incremental pull queries. It is used for warning decisions, skip
logic, targeted lookups, full-root push discovery, and incremental pull
filtering. It is updated after successful `pull` and `push` writes, and it is
not written during `--dry-run`.

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
2. the command-specific profile template, such as `[profiles.work.pull].template`
3. the shared profile template at `[profiles.work].template`
4. `./template.md` in your current working directory
5. `template.md` next to the installed binary

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

Internally, LinearSync also tracks each issue's `updatedAt` value in the local
sync cache so it can detect remote-only changes and avoid overwriting newer
remote state.

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
- Incremental pull uses per-team scan timestamps, but `push` still fetches remote issues note-by-note and does not yet short-circuit remote checks from cache state alone
- There are no explicit cache control flags yet such as `--rebuild-cache` or `--no-cache`
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

