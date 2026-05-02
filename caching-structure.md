# Caching structure plan

This document sketches a caching design for `LinearSync` that supports:

- faster `pull` and `push` as the note collection grows
- default exclusion of already-done issues/notes
- directional warnings when local changes need a `push` or remote changes need a `pull`
- future incremental sync based on Linear `updatedAt`

## Summary

Use a local cache file to store per-note sync baselines.

Status: phases 1 and 3 of this plan are now implemented, and phase 2 is
mostly implemented. The current code uses a note-root-local cache file,
records per-issue baselines, stores a local note index, records per-team remote
scan timestamps, fetches Linear `updatedAt`, computes a normalized local push
hash, and uses the derived sync state for warnings and skip decisions.

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

Current implementation path:

- `<note-root>/.linear-sync/cache.json`

Current implementation structure:

```json
{
  "version": 1,
  "last_local_index_at": "2026-04-29T12:00:00Z",
  "teams": {
    "team-id": {
      "last_remote_scan_at": "2026-04-29T12:00:00Z"
    }
  },
  "notes": {
    "ENG-123": {
      "path": "in-progress/ENG-123.md",
      "team_slug": "platform",
      "status_slug": "in-progress"
    }
  },
  "issues": {
    "ENG-123": {
      "path": "in-progress/ENG-123.md",
      "team_slug": "platform",
      "status_slug": "in-progress",
      "linear_id": "uuid-if-known",
      "last_sync_at": "2026-04-29T12:00:00Z",
      "last_synced_linear_updated_at": "2026-04-29T11:58:12Z",
      "last_synced_local_push_hash": "fnv1a64:8f2c..."
    }
  }
}
```

This keeps one cache per note root instead of multiplexing multiple roots into a
single file.

### Required fields

Per issue:

- `path`
- `team_slug`
- `status_slug`
- `last_sync_at`
- `last_synced_linear_updated_at`
- `last_synced_local_push_hash`

### Optional fields

Currently implemented:

- `linear_id`
- root-level `last_local_index_at`
- root-level `notes` note index entries

Potential future fields:

- `last_seen_local_mtime_unix`
- `last_seen_file_size`
- `last_conflict_at`
- `last_warning_kind`

Currently implemented per-team field:

- `last_remote_scan_at`

### Why one cache file per note root?

This keeps the cache location explicit and predictable, avoids cross-root
collisions, and matches the current command model where `pull` and `push` act on
one note root at a time.

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

Any stable hash is fine.

Current implementation uses a stable FNV-1a 64-bit digest over the canonical
JSON payload.

Example:

- `fnv1a64:abcd1234...`

If we later want a stronger or more self-describing digest, this can be swapped
without changing the overall cache model.

---

## Pull query strategy

There are two stages to the pull strategy.

### Stage 1: baseline improvement

Status: implemented.

Keep the current pull shape, but add `updatedAt` to fetched issues and use cache decisions locally.

That gives:

- smarter skipping
- done issue handling
- warnings when local changes need a `push`

### Stage 2: incremental remote pull

Status: implemented.

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

Implemented with a 5-minute overlap.

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
4. If there is no cache baseline:
   - treat the note as unknown state
   - fetch the remote issue before allowing any push decision
5. If `current_local_push_hash == last_synced_local_push_hash`:
   - local pushable metadata has not changed since the last synced baseline
   - this is the main case where remote work may be skipped in a future optimization
   - skip pushing unless explicit validation is needed
6. If `current_local_push_hash != last_synced_local_push_hash`:
   - always fetch the remote issue including `updatedAt`
   - compare current remote `updatedAt` against `last_synced_linear_updated_at`
   - if remote unchanged: normal push candidate
   - if remote changed too: conflict warning and do not overwrite
7. Otherwise:
   - continue with the normal diff / optional forced push flow
8. After a successful push, or after successfully persisting a note with a clean
   synced state:
   - refresh or trust authoritative remote `updatedAt`
   - update cache baselines

Safety rule: any note that is actually going to be pushed must fetch the remote
issue first. Local hash divergence alone is not enough to prove the remote issue
is unchanged.

This optimization is now implemented for unchanged local notes.

Current behavior:

- if the current local push hash matches `last_synced_local_push_hash`, `push`
  skips the remote fetch and does no further work for that note during full-root
  non-forced runs
- if `--issue-id` or `--force` is used, `push` still fetches the remote issue
  even when the local hash is unchanged so it can validate the cached baseline
  against current Linear state
- if the local push hash changed since the last synced baseline, `push` fetches
  the remote issue, computes the frontmatter and managed-block diffs, and writes
  the push review block back into the note when differences or warnings remain

### Push warnings

Currently implemented:

- remote changed since sync but local did not
- both local and remote changed since sync
- unchanged local notes short-circuit from the cached push hash without a remote fetch during full-root non-forced runs
- targeted and forced pushes always validate the cached baseline against current remote state

Desired safety behavior for future push short-circuiting:

- if the local push hash changed since the last synced baseline, fetch the
  remote issue before allowing any push
- never treat a locally changed note as safe to push without checking whether
  the remote issue changed too

Not yet implemented from this plan:

- warning when a local note has no synced baseline
- warning when a local note path disagrees with an expected status path

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

Currently implemented:

- local note changed since last sync but remote did not
- both local and remote changed since last sync
- local note is in the wrong status directory

Current behavior for invalid cache paths:

- if a cached path is missing, treat the cache entry as stale and fall back to
  directory discovery

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

Status: complete.

Added:

- `updatedAt` to relevant remote issue fetches
- cache file structs
- local push hash computation
- baseline comparison logic

Currently used for:

- pull-side warnings
- push-side warnings
- skipping unchanged pulls
- cached path lookup before directory scanning

Implemented since this phase summary was first written:

- push-side short-circuiting that skips remote fetches for unchanged local
  notes

### Phase 2: local note indexing

Mostly complete.

The current cache stores:

- identifier -> path
- identifier -> status slug
- identifier -> team slug
- root-level `last_local_index_at` freshness metadata

Current use:

- `pull` uses the cached path before scanning other status dirs
- `push --issue-id <ID>` uses the cached path before falling back to directory scanning
- full-root `push` uses the cached note index when it is fresh
- full-root `push` rebuilds the note index from disk when local directories changed

Still useful next:

- rely more heavily on the cache as a local note index in more workflows
- add explicit rebuild / bypass controls for local indexing

### Phase 3: incremental remote pull

Status: complete.

Added per-team:

- `last_remote_scan_at`

Implemented:

- query Linear with `updatedAt.gte`
- order by `updatedAt`
- paginate until complete
- use an overlap window

Current scope notes:

- scan markers are updated after non-dry-run team pulls
- targeted single-issue pulls do not advance the team scan marker

### Phase 4: explicit cache controls

Potential future flags:

- `--rebuild-cache`
- `--no-cache`
- `--cache-path <PATH>`

---

## Open questions

### 1. Where should the cache live?

Current decision:

- note-root-local: `<root>/.linear-sync/cache.json`

Reason:

- it stays close to the notes it describes
- it works cleanly with multiple note roots
- it keeps path resolution simple and predictable

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

Current decision:

- not yet

Future recommendation:

- yes, as optional validation/debugging metadata
- no, as the primary sync decision signal

---

## Recommendation

The current implementation is a good phase-1 foundation.

### Implemented now

- per-issue cache baselines
  - `last_synced_linear_updated_at`
  - `last_synced_local_push_hash`
  - `last_sync_at`
- `updatedAt` in relevant issue fetches
- warning logic based on local-only / remote-only / both-changed states
- cached note path metadata in the same file

### Useful next

- stronger cache validation and rebuild controls
- stronger cache validation and rebuild controls
- explicit cache controls such as rebuild / bypass flags

This already improves safety and pull efficiency, and it sets up a clean path
to faster remote scans later.
