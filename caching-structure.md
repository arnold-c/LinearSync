# Caching structure plan

This document sketches a caching design for `LinearSync` that supports:

- faster `pull` and `push` as the note collection grows
- default exclusion of already-done issues/notes
- directional warnings when local changes need a `push` or remote changes need a `pull`
- future incremental sync based on Linear `updatedAt`

## Summary

Use a local cache file to store per-note sync baselines.

The key idea is to track:

- the last synced remote modification timestamp from Linear: `updatedAt`
- the last synced hash of push-relevant local frontmatter

Then classify each issue/note pair at runtime as:

- neither changed
- local changed only
- remote changed only
- both changed

This enables:

- `pull` to warn when a `push` is needed
- `push` to warn when a `pull` is needed
- conflict detection when both sides changed
- skipping unchanged items
- future remote incremental queries using `IssueFilter.updatedAt`

---

## Schema confirmation

Linear's GraphQL schema supports this design.

### Issue fields

`Issue` includes:

- `updatedAt: DateTime!`
- `createdAt: DateTime!`

The schema description for `updatedAt` says it is:

> The last time at which the entity was meaningfully updated.

That makes it the right remote-side change signal.

### Issue filtering

`IssueFilter` includes:

- `updatedAt: DateComparator`
- `createdAt: DateComparator`
- `completedAt: NullableDateComparator`
- `state: WorkflowStateFilter`
- `team: TeamFilter`

`DateComparator` supports:

- `eq`
- `gt`
- `gte`
- `in`
- `lt`
- `lte`
- `neq`
- `nin`

This should allow incremental remote queries such as:

- issues updated after a given timestamp
- issues completed after a given timestamp
- issues in a given team updated since the last scan

### Ordering and pagination

Issue connections support:

- `first`
- `after`
- `orderBy`
- `filter`

`PaginationOrderBy` includes:

- `createdAt`
- `updatedAt`

`PageInfo` includes:

- `endCursor`
- `hasNextPage`

This supports a paginated incremental pull loop based on `updatedAt`.

---

## Why not use only file mtimes?

A local file mtime is useful as a weak hint, but not as the primary change signal.

Problems with using only mtime:

- editing `My notes` changes mtime but should not trigger a `push`
- `pull` writes the file and changes mtime even when no pushable metadata changed
- editor/OS behavior can change mtime in noisy ways

Instead, compute a normalized hash of only the frontmatter fields that `push` cares about.

Current pushable fields:

- `title`
- `status`
- `priority`
- `project`
- `tags`

Optional: still store the file mtime as a secondary signal for cache validation/debugging.

---

## Core sync model

For each local note / Linear issue pair, store the last successful sync baselines.

### Stored baselines

- `last_synced_linear_updated_at`
- `last_synced_local_push_hash`
- `last_sync_at`

### Runtime values

- `current_linear_updated_at`
- `current_local_push_hash`

### Derived state

- `remote_changed = current_linear_updated_at != last_synced_linear_updated_at`
- `local_changed = current_local_push_hash != last_synced_local_push_hash`

### Outcome table

| local_changed | remote_changed | Meaning | `pull` behavior | `push` behavior |
|---|---|---|---|---|
| false | false | fully synced | skip | skip |
| true | false | local changed only | warn: run `push` | push candidate |
| false | true | remote changed only | pull candidate | warn: run `pull` |
| true | true | both changed | conflict / manual review | conflict / manual review |

This is the core decision model for cached sync.

---

## Cache file shape

Start with a JSON file.

Suggested path:

- `.linear-sync/cache.json`

Suggested structure:

```json
{
  "version": 1,
  "roots": {
    "/absolute/path/to/linear-issues": {
      "generated_at": "2026-04-29T12:00:00Z",
      "teams": {
        "platform": {
          "last_remote_scan_at": "2026-04-29T12:00:00Z"
        }
      },
      "issues": {
        "ENG-123": {
          "path": "platform/in-progress/ENG-123.md",
          "team_slug": "platform",
          "status_slug": "in-progress",
          "linear_id": "uuid-if-known",

          "last_sync_at": "2026-04-29T12:00:00Z",
          "last_synced_linear_updated_at": "2026-04-29T11:58:12Z",
          "last_synced_local_push_hash": "sha256:8f2c...",

          "last_seen_local_mtime_unix": 1714392000,
          "last_seen_file_size": 4312
        }
      }
    }
  }
}
```

### Required fields

Per issue:

- `path`
- `team_slug`
- `status_slug`
- `last_sync_at`
- `last_synced_linear_updated_at`
- `last_synced_local_push_hash`

### Optional fields

- `linear_id`
- `last_seen_local_mtime_unix`
- `last_seen_file_size`
- `last_conflict_at`
- `last_warning_kind`

### Why key by absolute root?

This allows multiple different note roots to coexist without collisions.

---

## Local push hash design

The local push hash should be computed from a normalized subset of frontmatter.

### Included fields

- `title`
- `status`
- `priority`
- `project`
- `tags`

### Normalization rules

- trim scalar strings
- use consistent null handling
- sort `tags`
- normalize `tags` to a stable list of strings
- serialize in a stable key order

### Example canonical payload before hashing

```json
{
  "priority": "High",
  "project": "Sync Improvements",
  "status": "In Progress",
  "tags": ["bug", "sync"],
  "title": "Fix sync bug"
}
```

### Hash algorithm

Any stable hash is fine. A SHA-256 hex digest is easy to reason about.

Example:

- `sha256:abcd1234...`

---

## Pull query strategy

There are two stages to the pull strategy.

### Stage 1: baseline improvement

Keep the current pull shape, but add `updatedAt` to fetched issues and use cache decisions locally.

That gives:

- smarter skipping
- done issue handling
- warnings when local changes need a `push`

### Stage 2: incremental remote pull

Once the cache is established, switch to querying only remotely changed issues.

Suggested query shape:

```graphql
query IssuesUpdatedSince($teamId: String!, $cursor: String, $since: DateTimeOrDuration!) {
  issues(
    first: 100
    after: $cursor
    orderBy: updatedAt
    filter: {
      team: { id: { eq: $teamId } }
      updatedAt: { gte: $since }
    }
  ) {
    nodes {
      id
      identifier
      title
      url
      description
      updatedAt
      priority
      state {
        name
      }
      labels {
        nodes {
          name
        }
      }
      project {
        name
      }
      attachments {
        nodes {
          title
          url
        }
      }
    }
    pageInfo {
      endCursor
      hasNextPage
    }
  }
}
```

### Notes on this query

- query from root `issues(...)`, not only `team(id).issues(...)`, if that makes incremental filtering easier
- filter by team and `updatedAt`
- order by `updatedAt`
- paginate using `pageInfo.endCursor`

### Overlap window

Do not query from the exact last scan timestamp only.

Use a small overlap window to avoid missing updates due to:

- clock skew
- boundary timing
- partial failures

Example:

- if `last_remote_scan_at = T`
- query with `updatedAt.gte = T - 5 minutes`

Then deduplicate issue IDs in memory during the run.

---

## Push flow design

`push` should remain note-driven locally, but use cache baselines before doing full remote work.

### Push algorithm sketch

1. Load cache for the input root.
2. Enumerate candidate notes.
   - by default, skip `done/`
   - include them when `--include-done` is set
3. For each note:
   - parse local note
   - compute `current_local_push_hash`
   - load cache entry if present
4. If `current_local_push_hash == last_synced_local_push_hash`:
   - local pushable metadata has not changed
   - skip unless we explicitly need to validate against a remote change
5. If local hash changed:
   - fetch remote issue including `updatedAt`
   - compare to `last_synced_linear_updated_at`
   - if remote unchanged: normal push candidate
   - if remote changed too: conflict warning
   - if remote changed only: warn to run `pull`
6. After successful push:
   - refresh or trust authoritative remote `updatedAt`
   - update cache baselines

### Push warnings

Warn when:

- local note has no synced baseline
- remote changed since sync but local did not
- both local and remote changed since sync
- local note path disagrees with expected status path

---

## Pull flow design

`pull` should use remote issue state first, then compare against local baselines.

### Pull algorithm sketch

1. Load cache for the output root.
2. Fetch issues from Linear, including `updatedAt`.
3. For each issue:
   - resolve desired path
   - determine current existing path if the note already exists elsewhere
   - compute whether done issue should be processed
   - load cached baseline if present
4. If no local note exists:
   - create note if not skipped by done rules
   - create cache baseline after write
5. If local note exists:
   - parse note
   - compute `current_local_push_hash`
   - compare with cached baseline
   - compare remote `updatedAt` with cached baseline
6. Use the 4-way state table:
   - neither changed: skip
   - remote only: pull/update
   - local only: warn to run `push`
   - both changed: conflict warning
7. After successful pull:
   - update cache baselines

### Pull warnings

Warn when:

- local note changed since last sync but remote did not
- both local and remote changed since last sync
- local note is in the wrong status directory
- cache entry points to a missing file

---

## Done issue behavior with cache

The cache should preserve the current default behavior:

- by default, already-done issues are not processed if:
  - they already exist in `done/`, or
  - they do not exist locally
- issues transitioning to `Done` are always processed
- `--include-done` overrides the default skip behavior

### Cache help here

The cache allows quick decisions like:

- this issue is already known in `done/` and has no remote update since last sync → skip
- this issue used to be active but is now `Done` → process
- this done note exists locally and is being explicitly included → process

---

## Cache lifecycle and invalidation

Start with a conservative cache lifecycle.

### Cache loading

On command start:

- load cache if it exists
- if missing, initialize empty cache
- if version mismatch, ignore and rebuild

### Cache validation

A cache entry is suspicious if:

- `path` no longer exists
- issue file stem no longer matches identifier
- root path has changed

If a specific entry is invalid:

- treat it as missing
- refresh it opportunistically from the file system during the run

### Cache updates

After every successful `pull` write or `push` update:

- recompute the local push hash from the final local file state
- record the final known status/path
- update sync baselines

### Full rebuilds

Allow a full rebuild when:

- user passes a future `--rebuild-cache`
- cache version changes
- too many cache entries are invalid

---

## Conflict and warning policy

The cache is most useful when it produces clear action guidance.

### Pull-side warnings

#### Local changed, remote unchanged

Message shape:

> Local pushable metadata changed since the last sync, but Linear has not changed. Run `push` to update Linear before pulling.

#### Both changed

Message shape:

> Both the local note and the Linear issue changed since the last sync. Reconcile manually, then run `push` or `pull --force` as appropriate.

### Push-side warnings

#### Remote changed, local unchanged

Message shape:

> Linear changed since the last sync, but the local pushable metadata has not. Run `pull` before pushing.

#### Both changed

Message shape:

> Both the local note and the Linear issue changed since the last sync. Reconcile manually before pushing.

---

## Suggested implementation phases

### Phase 1: schema-aware cache foundation

Add:

- `updatedAt` to all relevant remote issue fetches
- cache file structs
- local push hash computation
- baseline comparison logic

Use this first for:

- warnings
- skip unchanged notes/issues
- done issue decisions

Do not yet switch to incremental remote fetching.

### Phase 2: local note indexing

Optionally expand the cache to act as a note index:

- identifier -> path
- identifier -> status slug
- identifier -> team slug

Use it to reduce directory traversal for:

- `push`
- locating notes in other status dirs

### Phase 3: incremental remote pull

Add per-team:

- `last_remote_scan_at`

Then:

- query Linear with `updatedAt.gte`
- order by `updatedAt`
- paginate until complete
- use overlap window

### Phase 4: explicit cache controls

Potential future flags:

- `--rebuild-cache`
- `--no-cache`
- `--cache-path <PATH>`

---

## Open questions

### 1. Where should the cache live?

Options:

- repo-local: `.linear-sync/cache.json`
- note-root-local: `<root>/.linear-sync/cache.json`
- user cache dir

Recommendation:

- store it near the note root or in repo-local `.linear-sync/`
- make location explicit and predictable

### 2. Should cache metadata ever be embedded in notes?

Recommendation:

- no

Reason:

- sync state is operational metadata, not note content
- embedding it in notes creates noisy file changes
- cache data is better kept separate

### 3. Should first-run behavior be warning-heavy?

On first run there is no baseline.

Recommendation:

- avoid strong conflict warnings until a sync baseline exists
- treat missing baselines as unknown state
- establish baselines after a successful write/update

### 4. Should we store local mtime too?

Recommendation:

- yes, as optional validation/debugging metadata
- no, as the primary sync decision signal

---

## Recommendation

Implement the cache in two layers:

### Required now

- per-issue cache baselines
  - `last_synced_linear_updated_at`
  - `last_synced_local_push_hash`
  - `last_sync_at`
- `updatedAt` in all issue fetches
- warning logic based on local-only / remote-only / both-changed states

### Useful next

- note index fields in the same cache
- per-team `last_remote_scan_at`
- incremental pull via `IssueFilter.updatedAt`

This gives the biggest benefit with manageable complexity and sets up a clean path to faster syncs later.
