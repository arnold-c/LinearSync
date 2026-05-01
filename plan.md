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

Still to do:
1. Propagate `Result` through remaining helpers where it improves
   clarity
   - prefer `?` for fallible file and network operations when the
     caller should decide how to handle failure
   - review pull/push write paths that still print and continue
   - keep only the top-level runtime responsible for final process exit

## Recommended order

1. remaining helper cleanup in pull/push flows
2. boundary cleanup in `src/lib.rs` / `src/main.rs` if needed

## Notes

- Keep changes small and reviewable.
- Preserve current behavior unless required for propagation.
- Prefer structural change before deeper behavioral refactors.
- Do not split `push_note()` yet beyond what is necessary to support
  compilation and structured error flow.
