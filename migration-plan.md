# LinearSync module migration plan

## Goals

- Split `src/main.rs` into a small set of focused modules.
- Keep the first pass as low-risk as possible.
- Prefer moving existing code with minimal behavior changes.
- Defer optional cleanups, deduplication, and deeper refactors until after the code is physically separated.

## Agreed constraints

- Use `reconcile.rs` rather than `merge.rs`.
- Put `prompt.rs` under the CLI area rather than under output.
- Do **not** split `pull_issues()` yet.
- Do **not** split `push_note()` yet.
- In the first pass, move the minimum number of things possible.
- After the initial move, focus on optional improvements separately.

## Recommended first-pass layout

```text
src/
  main.rs
  lib.rs

  cli/
    mod.rs
    args.rs
    prompt.rs

  linear/
    mod.rs
    client.rs
    models.rs

  notes/
    mod.rs
    frontmatter.rs
    render.rs
    sections.rs
    reconcile.rs
    discovery.rs
    paths.rs

  app/
    mod.rs
    pull.rs
    push.rs

  output/
    mod.rs
    diff.rs
```

This is intentionally more modular than a single-file layout, but still conservative:
- no filesystem abstraction yet
- no trait-based API layer yet
- no forced decomposition of large orchestration functions yet
- no major behavioral changes required for the initial migration

## Minimum-move first pass

The guiding rule for the first pass is:

> Move cohesive groups of code into modules, keep function signatures mostly intact, and avoid redesigning internals unless necessary for compilation.

### 1. `src/main.rs` - DONE

Keep only:
- dotenv/bootstrap
- CLI parsing
- API key loading
- construction of the HTTP client
- dispatch into pull/push entrypoints

Everything else should move out.

### 2. `src/lib.rs` - DONE

Create a library entrypoint that re-exports the modules needed by `main.rs`.

Completed:
- created `src/lib.rs`
- moved the existing application entrypoint into `linear_sync::run()`
- updated `src/main.rs` to be a thin binary wrapper calling the library
- kept the first pass conservative by leaving most code in `lib.rs` for now

Initial purpose:
- define module tree
- expose app entrypoints
- centralize shared internal visibility

This does **not** need to introduce a new `run()` abstraction immediately.

## Module-by-module move plan

### `src/cli/args.rs`

Move:
- `Cli`
- `Commands`
- `ForceSelection`
- `parse_force_selection`

Reason:
- keeps clap-facing types together
- isolates argument parsing from core logic

### `src/cli/prompt.rs`

Move:
- `PullSelection`
- `resolve_pull_selection`
- `prompt_for_pull_selection`
- `prompt_for_merge_all_teams`
- `prompt_for_output_dir`

Reason:
- these are CLI interaction concerns
- this fits better under `cli` than `output`

### `src/linear/models.rs` - DONE

Move:
- `TeamInfo`
- `PriorityInfo`
- `WorkflowState`
- `LabelInfo`
- `ProjectInfo`
- `RemoteIssue`
- `get_priority_label`
- `get_priority_number`

Completed:
- created `src/linear/models.rs`
- moved the Linear domain structs and priority helper functions out of `src/lib.rs`
- added the `linear::models` module and updated imports in `src/lib.rs`
- kept behavior unchanged and visibility conservative with `pub(crate)`

Reason:
- these are domain types used across pull, push, and rendering

### `src/linear/client.rs`

Move:
- `fetch_teams`
- `fetch_priority_values`
- `fetch_remote_issue_for_note`
- `fetch_remote_issue_by_identifier`
- `fetch_remote_issue_by_issue_v2_identifier`
- `fetch_remote_issue_by_team_and_number`
- `fetch_remote_issue_by_id`
- `fetch_project_by_name`
- `update_linear_issue`
- `graphql_request_result`
- `graphql_request`
- `fetch_required_issue`
- `parse_remote_issue`
- `parse_issue_identifier`
- `is_graphql_shape_error`
- `resolve_state`
- `resolve_label`
- `LINEAR_API_URL`

Reason:
- keeps all Linear transport and parsing in one place
- this is already a natural seam in the current file

Note for first pass:
- keep the existing function-style API if that is easiest
- do **not** require introducing a `LinearClient` struct yet
- do **not** normalize all error handling yet unless needed for module boundaries

### `src/notes/frontmatter.rs` - DONE

Move:
- `extract_linear_id_from_frontmatter`
- `normalize_frontmatter_key`
- `yaml_string`
- `yaml_string_list`
- `normalize_project_name`
- `override_frontmatter_value`
- `collect_frontmatter_keys`
- `extract_ignored_properties`
- `parse_frontmatter_map`
- `render_yaml_value_diff`
- `render_modified_yaml_value_diff`
- `yaml_scalar_for_diff`

Completed:
- created `src/notes/frontmatter.rs`
- moved the YAML/frontmatter parsing, normalization, and diff helpers out of `src/lib.rs`
- added the `notes::frontmatter` module and updated imports in `src/lib.rs`
- kept the move low-risk by continuing to call `split_frontmatter()` from `src/lib.rs`

Reason:
- cohesive YAML/frontmatter utilities

### `src/notes/render.rs`

Move:
- `TemplateContext`
- `RenderedNote`
- `load_template`
- `default_template_path_if_present`
- `default_template_search_paths`
- `initialize_installed_template_path`
- `installed_template_path`
- `render_template`
- `description_section_from_text`
- `labels_yaml_from_names`
- `github_links_yaml_from_urls`
- `render_remote_issue_note`
- `default_markdown_content`
- `DEFAULT_TEMPLATE_PATH`
- `INSTALLED_TEMPLATE_PATH`

Reason:
- template loading and note rendering belong together

### `src/notes/sections.rs` - DONE

Move:
- `ensure_managed_section`
- `split_frontmatter`
- `ManagedSectionWarning`
- `PushSyncWarning`
- `NoteLocationWarning`
- `insert_or_remove_generated_section`
- `insert_or_remove_note_location_warning`
- `insert_or_remove_push_sync_section`
- `extract_managed_section_body`
- `extract_managed_section`
- `extract_section`
- `remove_section`
- section marker constants:
  - `MANAGED_SECTION_START`
  - `MANAGED_SECTION_END`
  - `CONFLICT_SECTION_START`
  - `CONFLICT_SECTION_END`
  - `PUSH_SYNC_SECTION_START`
  - `PUSH_SYNC_SECTION_END`
  - `NOTE_LOCATION_SECTION_START`
  - `NOTE_LOCATION_SECTION_END`

Completed:
- created `src/notes/sections.rs`
- moved managed-section markers, section warning structs, and section insertion/extraction helpers out of `src/lib.rs`
- added the `notes::sections` module and updated imports in `src/lib.rs`
- kept the first pass conservative by continuing to reference `crate::FrontmatterWarning` from the new module

Reason:
- these are document-section mechanics

### `src/notes/reconcile.rs`

Move:
- `MergeResult`
- `FrontmatterWarning`
- `merge_with_existing_note`
- `merge_frontmatter`
- `insert_or_remove_conflict_section`
- `frontmatter_conflict_warning`
- `extract_user_content`
- `normalize_managed_section_for_diff`
- `render_text_diff`
- `managed_section_warning`
- `push_frontmatter_diff_warning`

Reason:
- this is the local/remote note reconciliation layer
- `reconcile.rs` is a better name than `merge.rs` because the code does more than merging

Note for first pass:
- keep the existing algorithms intact
- do not try to deduplicate conflict/diff logic yet

### `src/notes/discovery.rs`

Move:
- `LocalNote`
- `discover_markdown_notes`
- `discover_markdown_notes_for_issue`
- `parse_local_note`
- `is_done_directory`
- `include_done_issue`
- `find_issue_note_in_other_status`

Reason:
- local note discovery and parsing belong together

### `src/notes/paths.rs`

Move:
- `status_slug`
- `note_location_warning`
- `final_note_path_after_push`
- `write_note_to_path`
- `file_path_for_issue`
- `default_output_root`
- `default_output_root_for_all_teams`
- `default_output_dir_for_team`
- `slugify_team_name`
- `DEFAULT_OUTPUT_ROOT`

Reason:
- path and status-folder rules are cohesive

Important note:
- even though `write_note_to_path()` performs I/O, it is tightly coupled to note path semantics, so keeping it here is acceptable for the first pass

### `src/output/diff.rs`

Move:
- `print_colored_diff`
- `print_push_diff`
- `format_delta_patch`
- `print_delta_output`
- `print_yaml_style_diff_from_delta_output`
- `parse_delta_patch_header`
- `normalize_delta_fallback_line`
- `print_diff_with_delta`
- ANSI constants:
  - `ANSI_RED`
  - `ANSI_GREEN`
  - `ANSI_YELLOW`
  - `ANSI_BLUE`
  - `ANSI_RESET`

Reason:
- terminal diff rendering is an output concern

### `src/app/pull.rs`

Move:
- `PullStats`
- `pull_command`
- `pull_issues`
- `ALL_TEAMS_OPTION`

Reason:
- this is the pull workflow entrypoint and orchestration layer

Constraint for first pass:
- keep `pull_issues()` as one function for now
- only update imports/calls as needed to compile

### `src/app/push.rs`

Move:
- `PushStats`
- `IssueUpdatePlan`
- `push_command`
- `push_note`
- `build_issue_update_input`
- `resolve_force_keys`

Reason:
- this is the push workflow entrypoint and orchestration layer

Constraint for first pass:
- keep `push_note()` as one function for now
- only update imports/calls as needed to compile

## Suggested migration order

To minimize churn and compile breakage, move code in this order:

1. Create `src/lib.rs` and empty module files.
2. Move pure types/constants/helpers first:
   - `linear/models.rs`
   - `notes/frontmatter.rs`
   - `notes/sections.rs`
   - `notes/paths.rs`
3. Move rendering and diff output:
   - `notes/render.rs`
   - `output/diff.rs`
4. Move note reconciliation and discovery:
   - `notes/reconcile.rs`
   - `notes/discovery.rs`
5. Move CLI-specific code:
   - `cli/args.rs`
   - `cli/prompt.rs`
6. Move Linear API code:
   - `linear/client.rs`
7. Move orchestration last:
   - `app/pull.rs`
   - `app/push.rs`
8. Reduce `main.rs` to bootstrap and dispatch only.

This order keeps dependency direction relatively simple and avoids moving the highest-coupling functions first.

## What to avoid in the first pass

To keep the migration low-risk, explicitly avoid these changes initially:

- no redesign of public function signatures unless required
- no replacement of `process::exit` patterns yet
- no introduction of a custom error type yet
- no trait abstraction for the Linear API yet
- no splitting of `pull_issues()` yet
- no splitting of `push_note()` yet
- no broad deduplication passes yet
- no typed GraphQL response refactor yet
- no movement of tests into new files unless it becomes necessary to keep them compiling

## Follow-up improvements after the first pass

Once the code is compiling and behavior is unchanged, then tackle optional improvements.

### Priority follow-ups

1. Replace internal `process::exit` calls with `Result`-based error propagation.
2. Split `pull_issues()` into smaller helpers.
3. Split `push_note()` into smaller helpers.
4. Reduce duplication between `pull_issues()` note rendering and `render_remote_issue_note()`.
5. Introduce more structured app-layer result/report types instead of printing directly from business logic.
6. Consider a `LinearClient` struct or trait-backed API boundary.
7. Consider typed GraphQL response structs instead of raw `serde_json::Value` traversal.
8. Move tests closer to the modules they exercise.

## Success criteria for the first pass

The first-pass migration is successful if:

- `src/main.rs` becomes a thin entrypoint
- code compiles with behavior unchanged
- `pull_issues()` and `push_note()` remain intact
- module boundaries are clearer
- follow-up refactors become easier because concerns are no longer interleaved in a single file
