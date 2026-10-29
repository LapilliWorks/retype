# Platforms

Supported targets, delivery tiers and their consequences.

## 1. Support matrix

| Platform | Minimum | Architectures | Status |
|---|---|---|---|
| Linux | glibc 2.31, kernel 5.4 | x86_64, aarch64 | Supported |
| macOS | 13 Ventura | x86_64, aarch64 | Supported |
| Windows | 10 21H2 | x86_64, aarch64 | Supported |
| BSD | FreeBSD 13, OpenBSD 7 | x86_64 | Later, CLI only, no support commitment |
| WSL 2 | Current | x86_64 | Best effort, unsupported |
| Android, iOS | | | Out of scope |

Reasoning, not preference:

- Windows 10 21H2 is the floor for unpackaged toast notifications with an AppUserModelID
- macOS 13 is the floor for the notification API generation the notifier bundle uses, and it is old enough that no supported Mac is excluded
- glibc 2.31 covers Debian 11, Ubuntu 20.04 and RHEL 9, which is what a static-friendly CLI reasonably claims

## 2. Toolchain

| Item | Value |
|---|---|
| MSRV | Rust 1.85, required by edition 2024 |
| Pinned toolchain in CI | The stable channel at build time, plus one job on the MSRV |
| Edition | 2024 |

## 3. Filesystems

| Filesystem | Identity | Attributes | Notes |
|---|---|---|---|
| ext4, xfs, btrfs | device and inode | `user.*` | btrfs subvolumes report distinct device ids |
| APFS, HFS+ | device and inode | `com.*` | |
| NTFS, ReFS | volume serial and file id | stream, pending decision | |
| exFAT, FAT32 | volume serial and file id | none | Identity works on Windows; attributes are lost |
| Network shares | unreliable | unreliable | See `docs/watch.md` §9 |

Nothing in the product requires an attribute or a stream. Identity is the mechanism of record, and it works on every filesystem in the table that reports a stable identifier.

## 4. Path handling

| Item | Rule |
|---|---|
| Length | Long path awareness is enabled in the Windows manifest, and paths are passed with the `\\?\` prefix where the API requires it |
| Encoding | Paths are `OsStr` end to end; no lossy conversion outside diagnostics |
| Case sensitivity | Detected by probing at startup, not assumed per platform. Extension comparison is case-insensitive regardless |
| Unicode normalisation | Not performed. macOS reports NFD, and the product compares bytes rather than normalising |
| Reserved names | Windows device names are rejected as output names rather than producing an unusable file |

## 5. Delivery tiers

Two tiers, and the difference matters enough to be explicit in the documentation.

| Tier | How | Contains | Supported |
|---|---|---|---|
| Source | `cargo install`, or a Homebrew source formula | The CLI binary only | As a workaround, for people who build their own |
| Installed | Platform installer, signed | CLI, daemon, notifier bundle, URI scheme registration, service definition | Yes |

`cargo install` is **CLI only and a workaround**. It gives conversion, explanation and in-process running with no daemon and no watched directories that survive a reboot, because a bare binary cannot register a URI scheme on macOS or a Start-menu shortcut on Windows, and cannot install a user service. The documentation states this rather than implying feature parity.

What the installer does, per platform:

| Platform | Actions |
|---|---|
| Linux | Binary, notifier desktop entry, `retype://` via `xdg-mime`, systemd user unit (not enabled) |
| macOS | App bundle containing the CLI and the notifier, `LSBackgroundOnly` agent, launchd user agent, scheme registered in the bundle |
| Windows | Binary, Start-menu shortcut carrying the AppUserModelID, registry entries for `retype://`, optional startup entry |

None of these actions is performed implicitly by running the binary.

## 6. Signing

| Platform | Requirement | When |
|---|---|---|
| macOS | Developer ID, hardened runtime, notarisation for every distributed binary | Before the first published binary |
| Windows | Azure Trusted Signing or an equivalent certificate; SmartScreen reputation accrues to the publisher identity | Before the first published binary |
| Linux | Detached signatures and checksums; no central authority | With the first release artifact |

Publisher identity is fixed: Lapilli Works LLC, `com.lapilliworks.retype`. Reputation is bound to that identity, so it is never changed casually.

## 7. Notifications and scheme registration

Capability per platform, matching:

| Platform | Actions | Owner |
|---|---|---|
| Linux | libnotify actions | Daemon |
| Windows | Toast with protocol activation | Daemon, via the Start-menu shortcut identity |
| macOS | User Notifications with categories | Notifier bundle |

When the GUI is installed it registers `retype://` and the notifier bundle defers. When the daemon is not running, the scheme handler performs undo in-process where possible, and otherwise reports that the daemon is unavailable.

## 8. Out of scope

| Target | Why |
|---|---|
| 32-bit x86 | No population, and doubling the build matrix for nothing |
| macOS 12 and older | Well below current support, and fragments the notification code |
| Windows 8.1 and older | No modern toast API |
| Android, iOS | No rename surface to hang the product on |
| Network filesystems | Needs its own stage, per the Deferred table |
| Fonts, 3D, CAD formats | Not formats this product converts |
