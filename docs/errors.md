# Errors

One error type, one set of codes, one mapping to every surface.

## 1. Layers

| Layer | Produces | Examples |
|---|---|---|
| Invocation | Usage errors | Missing argument, unknown flag, conflicting flags |
| Grammar | Diagnostics with spans | `E_ANCHOR`, `E_INAPPLICABLE` |
| Plan | Registry and capability errors | No backend implements the plan, tier unavailable |
| Execute | Backend errors | Tool failed, encoder refused, output unreadable |
| Persist | Filesystem and state errors | Permission denied, no space, lock held, journal corrupt |
| Transport | IPC errors | Version mismatch, daemon unavailable, ambiguous prefix |
| Internal | Bugs | Invariant violations, panics caught at the boundary |

## 2. Anatomy

```rust
pub struct Error {
	pub code: Code,
	pub category: Category,
	pub message: String,
	pub detail: Option<serde_json::Value>,
	pub hint: Option<String>,
	pub span: Option<Span>,
	pub report: Option<String>,
}
```

- `code` is stable and never reused
- `message` is one sentence, naming the thing and what to do next
- `detail` carries machine-readable context for `--json`
- `span` exists only for grammar diagnostics
- `report` exists only for `internal`, and is what `retype doctor --report` bundles

## 3. Codes

Grammar diagnostics keep their unprefixed names, fixed by `retype-grammar-1`: `E_ANCHOR`, `E_CHARSET`, `E_INTENT_EMPTY`, `E_TOO_LONG`, `E_TOO_MANY`, `E_GROUP_EMPTY`, `E_TOKEN_EMPTY`, `E_FORMAT_UNKNOWN`, `E_FORMAT_FOLDER`, `E_NAME_UNKNOWN`, `E_VALUE_MISSING`, `E_VALUE_UNEXPECTED`, `E_VALUE_SYNTAX`, `E_VALUE_RANGE`, `E_DUPLICATE`, `E_EXCLUSIVE`, `E_META_CONFLICT`, `E_NOOP`, `E_INAPPLICABLE`, `E_UNSUPPORTED`.

Everything else is namespaced:

| Namespace | Codes |
|---|---|
| `E_USAGE_` | `ARGS`, `CONFLICT`, `OUTPUT_COUNT` |
| `E_CFG_` | `PARSE`, `VERSION`, `VALIDATION`, `MISSING`, `UNKNOWN_KEY`, `NAME_CLASH` |
| `E_TOOL_` | `MISSING`, `VERSION`, `FAILED`, `TIMEOUT` |
| `E_IO_` | `PERMISSION`, `SPACE`, `NOT_FOUND`, `EXISTS`, `LOCKED`, `CORRUPT_JOURNAL` |
| `E_JOB_` | `STATE`, `CANCELLED`, `INTERRUPTED`, `PARTIAL`, `SUPERSEDED`, `RESUME_UNAVAILABLE` |
| `E_WATCH_` | `PAUSED_BY_POLICY`: the directory file sets `paused = true` |
| `E_IPC_` | As defined in `docs/ipc.md` |
| `E_INTERNAL_` | `INVARIANT`, `PANIC`, `UNEXPECTED` |

## 4. Categories

| Category | Meaning | Exit | Retryable | Notification action | Journal state |
|---|---|---|---|---|---|
| `diagnosis` | The request cannot be satisfied as written | 3 | No | Tweak | `needs-attention` |
| `environment` | This machine cannot satisfy it | 4 | After a fix | Open doctor | `failed` |
| `transient` | Try again | 5, 6 | Yes | Retry | `failed`, marked retryable |
| `internal` | A bug | 1 | No | Report | `failed`, with a report id |

Exit codes are the ones in `docs/cli.md` §6. A category maps to one code, so a script can branch on the category without parsing messages.

Rules:

- A diagnosis never retries on its own. Retrying an invalid intent produces the same invalid intent
- A transient failure is retried at most once, with backoff, and then reported
- An environment failure is not retried automatically. It needs a human to install something
- An internal failure is not retried, always carries a report id, and is logged at error level

## 5. Partial completion and resume

Outputs are independent units of work. A job does not roll back, because re-running a multi-minute conversion because another output failed is worse for the user than continuing from where it stopped.

Rules:

- Each output has its own state: `pending`, `running`, `done`, `failed`, `skipped`
- A job whose outputs are mixed is `partial`
- A failed output leaves the others in place and is retried by `retype jobs resume`
- Resume skips outputs whose recorded identity still matches a file on disk, and re-runs the rest. `--force` re-runs everything
- Resume needs the source bytes, which come from the path itself while it still holds them, otherwise from the undo record. When neither is available the answer is `E_JOB_RESUME_UNAVAILABLE`, and the message says which two things are missing

The product's invariant is about paths, not jobs: a path never carries a format it does not contain. It holds at every instant, not only after commit: the source is renamed back to its own extension before a conversion starts (`docs/state.md` §5.4), and each output is written to a temporary file and renamed only when complete. The single case that could break it is a job in which no output succeeded, because then the renamed source still holds the original format. That case restores the source extension and fails the job, so the invariant holds in every branch.

## 6. Diagnostics

- Grammar diagnostics accumulate, up to 8, ordered by span offset
- The CLI prints the first diagnostic in full plus a count of the rest; `--json` prints all of them
- `retype explain` always prints all of them
- Suggestions appear as `hint`, generated from edit distance 2 against registered names

## 7. Surfaces

| Surface | What it shows |
|---|---|
| CLI stdout | Success lines only |
| CLI stderr | Errors, warnings, progress |
| `--json` | `{v, ok, error:{code, category, message, detail, hint, span, report}}` |
| Journal | `state` plus `error:{code, message}` |
| Notification | Category-specific action, one sentence |
| IPC | The same `code`, `category` and `detail` |
| GUI | Category decides between a dialog, an inline badge and a toast |

No surface invents a message. The daemon is the only place a message is composed, and every client renders what it was given.

## 8. Message rules

- Name the file or token that caused it: `"unsupported operation 'colors' for png"` and not `"invalid input"`
- State the remedy when one exists: `"install ImageMagick or drop '+quality80'"`
- No stack traces for `internal` unless `--verbose`; the report id is enough
- No trailing punctuation, no exclamation
- Never blame the user. "not supported" is a fact, "you asked for something impossible" is not
