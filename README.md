# Retype

Rename the extension, the file converts.

Status: pre-alpha. The engine is being rebuilt from specification; see and `docs/`.

## What it will be

A file watcher and a command line tool that convert a file when you rename its
extension. `photo.heic` becomes `photo.jpg`, and the bytes follow the name.
Transformations ride on the same gesture: `photo.heic` becomes `photo.webp+q80`.

- Cross platform: Linux, macOS, Windows
- Local only: no upload, no account, no telemetry
- Scriptable, with the same intent string accepted by the filename, the command
  line and the editor

## Documentation

| Document | Contents |
|---|---|
| `docs/grammar.md` | The intent grammar |
| `docs/operations.md` | Operations and their semantics |
| `docs/quality.md` | Per format meaning of quality and size dials |
| `docs/config.md` | Configuration schema and precedence |
| `docs/cli.md` | Command line surface and exit codes |
| `docs/ipc.md` | Daemon and client protocol |
| `docs/state.md` | Journal, undo store, identifiers |
| `docs/watch.md` | What is watched, ignored and primed |
| `docs/tools.md` | External tool support matrix |
| `docs/errors.md` | Error taxonomy |
| `docs/provenance.md` | What is recorded about a conversion |
| `docs/platforms.md` | Supported targets and delivery tiers |
| `docs/events.md` | Filesystem event behaviour per platform (generated) |
| `docs/fidelity.md` | Per pair fidelity claims (generated) |

## Installation tiers

Two tiers exist, and the difference is deliberate.

- **Installed**: platform installer, signed. Command line, daemon, notifier,
  URI scheme, user service. Supported
- **Source**: `cargo install`. Command line only, no daemon, no watched
  directories that survive a reboot. A workaround, not the supported path

## Licence

ISC. See `LICENSE`. The graphical application is a separate, proprietary
product and lives in its own repository.
