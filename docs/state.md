# State, journal and identifiers

On-disk state for jobs, undo and calibration.

## 1. Layout

State directory: `RETYPE_STATE_DIR` when set, otherwise `dirs::state_dir()` joined with `retype`, falling back to `dirs::data_local_dir()` when the former is absent, which is the case on macOS. Unix socket paths are length-limited, so a state directory that would overflow the limit is a startup error naming the limit and the variable.

```
retype/
  jobs.jsonl                  append-only journal
  jobs.jsonl.corrupt.<ts>     quarantined journal
  lock                        exclusive lock, held by the running daemon
  daemon.sock                 IPC endpoint on Unix; a named pipe on Windows
  watches.toml                registered directories
  bench.toml                  estimator calibration
  undo/
    <id>/
      meta.toml               record description
      source.<ext>            the original bytes, hard link or copy
```

Directory mode is `0700`, files `0600`. The store holds user data in plain form, so nothing here is world-readable.

## 2. Identifiers

A job id is a ULID: 26 characters, Crockford base32, 48-bit millisecond timestamp followed by 80 random bits. Properties required:

- Lexicographic order equals creation order
- Unique without coordination, so the job feed in needs no id registry
- Case-insensitive in practice, because base32 excludes `I`, `L`, `O` and `U`

The id is generated at job creation, before execution, so a deferred job is addressable by its notification immediately.

Machine identity is a separate per-install random field, stable across restarts, stored in `bench.toml`. It is not part of the id.

User-facing handle: a prefix of at least 4 characters. Resolution requires a unique match; an ambiguous prefix is an error listing the candidates. Listings print the shortest unambiguous prefix so copy-paste is short by default.

## 3. Journal records

One JSON object per line. Every line is self-contained and carries its own version.

```json
{"v":"retype-journal-1","kind":"job","id":"01J8Z...","created":1690000000123,"updated":1690000000456,
 "source":{"path":"/home/u/Pictures/a.heic","format":"heic","size":2431002,"mtime_ns":1690000000000},
 "intent":"jpg+quality85",
 "outputs":[{"path":"/home/u/Pictures/a.jpg","format":"jpg","state":"done","bytes":812345,"tier":"converted",
              "identity":{"kind":"unix","dev":2049,"ino":5243121,"birth_ns":1690000000000}},
            {"path":"/home/u/Pictures/a.png","format":"png","state":"failed","tier":"converted",
              "error":{"code":"E_TOOL_FAILED","message":"converter exited 1"}}],
 "state":"partial","backend":"magick","machine":"7f3c1a2b","error":null}
```

| Field | Meaning |
|---|---|
| `v` | Record schema identifier, `retype-journal-1` |
| `kind` | Record type; only `job` in version 1 |
| `id` | ULID |
| `created`, `updated` | Unix milliseconds |
| `source` | Path, format, size, mtime in nanoseconds |
| `intent` | Canonical intent string, or null for a plain conversion |
| `outputs` | Resolved outputs with format, per-output state, resulting size, tier, file identity and per-output error |
| `state` | `pending`, `queued`, `running`, `done`, `partial`, `failed`, `cancelled`, `superseded`, `needs-attention` |
| `backend` | Backend name that ran or will run the job |
| `machine` | Per-install identity |
| `error` | Null, or `{code, message}` with a journal-level code such as `E_INTERRUPTED`, `E_BACKEND_FAILED`, `E_DIAGNOSIS` |

A record is a full snapshot of the job at that transition, so replay is last-wins per id and no record depends on another. Snapshots are affordable because a job has a handful of transitions.

## 4. Journal rules

- Records are appended, never edited in place
- `fsync` on transitions into `pending` and `done`; other transitions are best effort, because reconciliation covers the gap
- Line order defines time; wall-clock fields are informational
- A trailing line that does not parse is a torn write and is dropped silently
- A non-trailing line that does not parse is corruption: the file is moved to `jobs.jsonl.corrupt.<ts>`, a new journal is started, and the event is reported once
- A line whose `v` is not the supported identifier is skipped and reported once, so a downgrade remains usable
- Adding fields is compatible within an identifier. Removing or renaming a field requires a new identifier, and the previous one stays readable for one major release

Compaction rewrites live records into a temporary file and renames it into place. It runs on startup when the journal exceeds 10,000 lines or when more than half the records are superseded by later ones for the same id. Records older than twice the undo retention, with a floor of 30 days, are dropped.

## 5. Undo records

```
undo/<id>/meta.toml
```

```toml
version = 'retype-undo-1'
id = '01J8Z...'
intent = 'jpg+quality85'
created = 1690000000123
machine = '7f3c1a2b'
source_path = '/home/u/Pictures/a.heic'
source_ext = 'heic'
source_size = 2431002
source_mode = 0o644
source_mtime_ns = 1690000000000
link = 'hard'

[[outputs]]
path = '/home/u/Pictures/a.jpg'
ext = 'jpg'
```

The source is a hard link when the source and the store share a filesystem, detected by device id, and a copy otherwise. `link` records which.

Undo restores the source path with `source_ext`, `source_mode` and `source_mtime_ns`, removes every output that was produced, and deletes the record. Undoing a `partial` job therefore removes the outputs that exist and leaves nothing behind. Restoring a copy sets mtime explicitly, so undo does not depend on the journal.

Records are the durable truth for recoverability. The journal is a cache of history and progress: losing it costs listing and progress, never the ability to undo. Reconciliation therefore reads the undo store first.

## 5.1 Watch registrations

```toml
version = 'retype-watches-1'

[[watch]]
path = '/home/u/Documents'
recursive = true
added = 1690000000123
```

The daemon owns this file. `watch` appends, `unwatch` removes, and the daemon re-reads it on start so registrations survive restarts and reboots. Hand editing is tolerated and validated like any other configuration file.

## 5.2 Resume

`partial` means at least one output succeeded and at least one failed or never started.

Resume input, in order:

1. The source path, if it still carries the recorded source identity, which is the case when no output has replaced it
2. The undo record for the job, which holds the original bytes as a hard link or a copy
3. Nothing, which is `E_JOB_RESUME_UNAVAILABLE`

Resume skips outputs whose recorded identity matches a file that still exists with the recorded size, and runs the rest. `--force` ignores the recorded identities and runs every output again.

Resume is therefore available for as long as the undo record survives retention, and unavailable afterwards. That coupling is deliberate rather than incidental: it makes retention a single, documented knob instead of two.

## 6. Startup reconciliation

1. Acquire the lock. Failure means another daemon owns the state directory, and the process exits with that message
2. Load `bench.toml`, then the journal, then the undo record index
3. For every journal job in `pending`, `queued` or `running`: if the source still exists and its ext and mtime match, requeue it. Otherwise mark it `failed` with `E_INTERRUPTED`
4. For every undo record without a journal entry, keep it and rebuild a `done` record from `meta.toml`
5. For every journal job whose record directory is missing, leave the state as recorded and mark undoing it as unavailable
6. Drop undo records past retention, then compact if the thresholds are met

Nothing is deleted during reconciliation. Anything inconsistent is reported and left in place for inspection.

## 7. Calibration

`bench.toml` holds per-machine measurements and the machine identity.

```toml
version = 'retype-bench-1'
machine = '7f3c1a2b'

[[throughput]]
family = 'image'
format = 'jpg'
backend = 'native'
megapixels_per_second = 12.4
```

Estimates use measured throughput when present and family constants otherwise. Every constant carries its provenance in code, with a source URL when it comes from outside this repository, and the measurement method recorded next to it.

## 8. Provenance

Mechanisms, guarantees and failure modes are specified in `docs/provenance.md`. This document owns only the field: the journal records the identity of every output at completion.

```json
"outputs":[{"path":"/home/u/Pictures/a.jpg","format":"jpg","bytes":812345,"tier":"converted",
            "identity":{"kind":"unix","dev":2049,"ino":5243121,"birth_ns":1690000000000}}]
```

```json
"identity":{"kind":"windows","volume_serial":"1A2B3C4D","file_id":"0000000000000000000000000001F4A3"}
```

`kind` is `unix` or `windows`. A record without `identity` cannot answer by identity, and `retype why` then falls back to path matching or reports unknown.
