# Result propagation follow-up plan

## Goal

Finish propagating structured `Result` types through the codebase so
library-style modules return errors instead of exiting directly, while
the top-level runtime remains responsible for final user-facing exit
behavior.

## Status

Completed:
- `src/linear/client.rs` now uses `AppError` instead of
  `Result<_, String>`.
- `src/cli/prompt.rs` no longer uses `expect()` and now returns
  `Result` for prompt I/O.
- `src/notes/discovery.rs::parse_local_note()` now returns
  `Result<_, AppError>`.
- `src/app/push.rs::push_note()` now uses structured helper functions
  for remote issue lookup and note persistence while preserving
  per-note warnings and reporting.
- top-level exit handling remains confined to `src/lib.rs`.
- remaining pull/push helper cleanup now routes fallible write and
  refetch operations through small `Result`-returning helpers.
- pull and push command code now decides whether to surface helper
  failures as warnings, notes, or fatal errors.
- no further boundary cleanup is currently needed in `src/lib.rs` or
  `src/main.rs`; top-level exit handling remains in the runtime.

Still to do:
- none currently identified for this result-propagation follow-up.

## Recommended order

- no further work planned for this follow-up.

## Notes

- Keep changes small and reviewable.
- Preserve current behavior unless required for propagation.
- Prefer structural change before deeper behavioral refactors.
- Do not split `push_note()` yet beyond what is necessary to support
  compilation and structured error flow.
