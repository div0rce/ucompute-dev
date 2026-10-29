# Rename to UCompute in code, files, paths and repositories (D-027, D-028)

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

In the tables below, `<former>` stands for the former name (D-030: it is not written out).

**D-028 (owner, 2026-10-29) supersedes the transition column below:** there is no compatibility layer. No old protocol
names, variables, headers or folder migration are kept; every device is reinstalled with renamed builds and paired
again once. Every current file stops using the former name, including test helper modules (`tests/<former>/` becomes
`tests/support/`) and the fixture folder (`.<former>/` becomes `.ucompute/`). GitHub: public `div0rce/ucompute` (the
renamed former public repository, nothing new pushed), private `div0rce/ucompute-dev` (origin). The local directory
moves to `/Users/nasr/repos/ucompute` without a symlink, last. The rest of this plan (what changes, the order) still
applies; D-030 supersedes "what stays historical" below.

D-021 kept code identifiers under the former name until after physical acceptance. On 2026-10-29 the owner asked to
complete the transition to ucompute across the repositories, file names, paths and everything else. This plan does it
without breaking the devices already running older builds (the owner's installed candidate 1, sentinel, the Windows
laptop) and without losing anyone's device identity or pairings.

## What changes

| Identifier | Old | New | Transition |
|---|---|---|---|
| Crates and their paths | `crates/<former>-*`, `<former>_*` | `crates/ucompute-*`, `ucompute_*` | none needed (source only) |
| CLI binary and sidecar | `<former>` | `ucompute` | scripts and docs updated in the same change |
| Data-folder variable | `<FORMER>_HOME` (and the other `<FORMER>_*` variables) | `UCOMPUTE_HOME` (`UCOMPUTE_*`) | the old names are still read, with a deprecation warning |
| Default data folders | `~/Library/Application Support/<Former>`, `%LOCALAPPDATA%\<Former>`, `$XDG_DATA_HOME/<former>` | `…/UCompute`, `…\UCompute`, `…/ucompute` | on first start, an existing old folder is moved to the new name when the new one does not exist, so the device identity, pairings, models and settings carry over |
| HTTP headers | `X-<Former>-*` | `X-UCompute-*` | both accepted; the new one is sent |
| Metrics | `<former>_*` | `ucompute_*` | none (no external dashboards exist) |
| Peer protocol ALPN | `<former>/1` | `ucompute/1` | both offered and accepted, the new one preferred, so new and old builds still connect |
| Pairing payload | `<former>-pair:v1:` | `ucompute-pair:v1:` | both accepted; the new one is produced |
| mDNS service | `_<former>._udp` | `_ucompute._udp` | both advertised and browsed, deduplicated by device id |
| Protobuf package | `<former>.v1` | `ucompute.v1` | wire bytes unchanged (field numbers only) |
| App, bundle and installers | `<Former>.app`, the former bundle id, `<Former>_<v>_*.dmg/-setup.exe` | `UCompute.app`, `org.ucompute.desktop`, `UCompute_<v>_*` | a new bundle id installs beside the old app; the app moves its own log folder; the old app is removed by hand |
| Linux unit and tarball | `<former>.service`, `<former>-<v>-linux-*` | `ucompute.service`, `ucompute-<v>-linux-*` | none |
| Repository directory | `/Users/nasr/repos/<former>` | `/Users/nasr/repos/ucompute` | a symlink from the old path, so other sessions, worktrees and references keep working |
| Test hosts | `~/src/<former>` (sentinel), `C:\Users\jessg\src\<former>` (laptop) | `…/ucompute` | a symlink / junction from the old path; the laptop's scheduled-task scripts are updated |
| GitHub | `div0rce/<former>` (public, untouched) | `div0rce/ucompute` (private, if authorized) | see `docs/release/github-sync-proposal-2026-10-29.md` |

The transition acceptances (old ALPN, pairing prefix, mDNS service, headers, variables) are removed only by a later
decision, once every device the owner uses runs a renamed build.

## What does not change

- Historical records (`docs/VERIFICATION.md` entries, `docs/PROGRESS.md`, `docs/evidence/*`, ADR history, decisions)
  keep the names they were written with: they describe what ran. *(Superseded by D-030: they use the current names,
  each with a one-line note, and the originals stay in git history.)*
- The approved candidate 1 (`<Former>_0.1.0_aarch64.dmg`, D-025) stays exactly that file; the next candidate is the
  first `UCompute_*` artifact and needs its own record and approval. *(D-030 withdraws candidate 1.)*
- Ledger row ids; the requirements ledger's paths are updated to the new crate paths.

## Order

1. Rust workspace (crates, binary, variables, folders with migration, headers, metrics, protocol identifiers with dual
   acceptance), with tests for each transition rule.
2. Desktop app (names, bundle id, sidecar, installers, log folder) and its verifiers.
3. Scripts, runners, packaging, current docs, AGENTS.md, the ledger's paths.
4. Whole-tree check on the Mac, the Linux suite on sentinel, Windows clippy; a scan for leftover old identifiers
   (allowed only in the transition code and historical records).
5. The repository directory and the test hosts' paths, last, with compatibility links.
