# Rename: Common → UCompute in code, files, paths and repositories (D-027)

D-021 kept code identifiers as `common` until after physical acceptance. On 2026-10-29 the owner asked to "complete the
transition from common --> ucompute with the repos and file name and path and all". This plan does it without
breaking the devices already running older builds (the owner's installed candidate 1, sentinel, the Windows laptop)
and without losing anyone's device identity or pairings.

## What changes

| Identifier | Old | New | Transition |
|---|---|---|---|
| Crates and their paths | `crates/common-*`, `common_*` | `crates/ucompute-*`, `ucompute_*` | none needed (source only) |
| CLI binary and sidecar | `common` | `ucompute` | scripts and docs updated in the same change |
| Data-folder variable | `COMMON_HOME` (and the other `COMMON_*` variables) | `UCOMPUTE_HOME` (`UCOMPUTE_*`) | the old names are still read, with a deprecation warning |
| Default data folders | `~/Library/Application Support/Common`, `%LOCALAPPDATA%\Common`, `$XDG_DATA_HOME/common` | `…/UCompute`, `…\UCompute`, `…/ucompute` | on first start, an existing old folder is moved to the new name when the new one does not exist, so the device identity, pairings, models and settings carry over |
| HTTP headers | `X-Common-*` | `X-UCompute-*` | both accepted; the new one is sent |
| Metrics | `common_*` | `ucompute_*` | none (no external dashboards exist) |
| Peer protocol ALPN | `common/1` | `ucompute/1` | both offered and accepted, the new one preferred, so new and old builds still connect |
| Pairing payload | `common-pair:v1:` | `ucompute-pair:v1:` | both accepted; the new one is produced |
| mDNS service | `_common._udp` | `_ucompute._udp` | both advertised and browsed, deduplicated by device id |
| Protobuf package | `common.v1` | `ucompute.v1` | wire bytes unchanged (field numbers only) |
| App, bundle and installers | `Common.app`, `org.commoncompute.desktop`, `Common_<v>_*.dmg/-setup.exe` | `UCompute.app`, `org.ucompute.desktop`, `UCompute_<v>_*` | a new bundle id installs beside the old app; the app moves its own log folder; the old app is removed by hand |
| Linux unit and tarball | `common.service`, `common-<v>-linux-*` | `ucompute.service`, `ucompute-<v>-linux-*` | none |
| Repository directory | `/Users/nasr/repos/common` | `/Users/nasr/repos/ucompute` | a symlink from the old path, so other sessions, worktrees and references keep working |
| Test hosts | `~/src/common` (sentinel), `C:\Users\jessg\src\common` (laptop) | `…/ucompute` | a symlink / junction from the old path; the laptop's scheduled-task scripts are updated |
| GitHub | `div0rce/common` (public, untouched) | `div0rce/ucompute` (private, if authorized) | see `docs/release/github-sync-proposal-2026-10-29.md` |

The transition acceptances (old ALPN, pairing prefix, mDNS service, headers, variables) are removed only by a later
decision, once every device the owner uses runs a renamed build.

## What does not change

- Historical records (`docs/VERIFICATION.md` entries, `docs/PROGRESS.md`, `docs/evidence/*`, ADR history, decisions)
  keep the names they were written with: they describe what ran.
- The approved candidate 1 (`Common_0.1.0_aarch64.dmg`, D-025) stays exactly that file; the next candidate is the
  first `UCompute_*` artifact and needs its own record and approval.
- Ledger row ids; the requirements ledger's paths are updated to the new crate paths.

## Order

1. Rust workspace (crates, binary, variables, folders with migration, headers, metrics, protocol identifiers with dual
   acceptance), with tests for each transition rule.
2. Desktop app (names, bundle id, sidecar, installers, log folder) and its verifiers.
3. Scripts, runners, packaging, current docs, AGENTS.md, the ledger's paths.
4. Whole-tree check on the Mac, the Linux suite on sentinel, Windows clippy; a scan for leftover old identifiers
   (allowed only in the transition code and historical records).
5. The repository directory and the test hosts' paths, last, with compatibility links.
