# Migrating every device to the UCompute names (D-028, D-029, D-030)

In the table below, `<former>` stands for the former name (D-030: it is not written out).

D-028 removes every compatibility path: renamed builds neither accept the old protocol identifiers nor read old
variables or folders. D-029 adds three conditions: no user data is deleted automatically, every supported machine
passes a full acceptance test, and the protocol identifiers are frozen independently of the product name. This is the
procedure. It starts once the rename pull request is merged and a renamed build exists for each platform.

## 1. Devices and what they hold

| Device | Old installation | Old data | Kept by default |
|---|---|---|---|
| Owner's Mac (M2) | `/Applications/<Former>.app` (candidate 1, D-025) and the previous app kept as `<Former>.app.prev` | `~/Library/Application Support/<Former>` (identity `8983-9a1e-e341`, pairings, groups, audit, models, engine); `~/Library/Logs/` under the former bundle id | models (re-hashed), settings; identity and pairings by choice (§3) |
| This repository's fixture home | `.<former>/` (renamed to `.ucompute/` by the rename itself, a move, nothing deleted) | engine archives, test models | everything |
| sentinel (Ubuntu) | `~/src/<former>` (engineering tree, `target/release/<former>`), the soak's `~/.<former>-soak`, `~/.<former>-acceptance`, `~/.<former>-test`, `~/soak` | the acceptance identity, models (stories15M, qwen2.5 0.5B q4_k_m and q8_0, 1.5B q4_k_m), engine | models (re-hashed); the soak results already collected |
| Windows laptop | `%LOCALAPPDATA%\Programs\<Former>` (593d4ea app), verifier installs under `<Former>Verify*`, `C:\Users\jessg\src\<former>`, scheduled tasks `<Former>Test*` | `%LOCALAPPDATA%\<Former>` (identity, pairings, models, engine) | models (re-hashed) |
| Kyle's Mac | `<Former>.app` (6ab9f8b) | its own data folder | migrated when it is next reachable, the same way |

## 2. Before reinstalling (condition 1)

1. Stop the old app or daemon; record its version and every running process, unit and scheduled task it owns.
2. Inventory the old data folder (each file, size and, for model blobs, SHA-256) into `migration-<device>-<date>/inventory.json`.
3. Back it up whole into `migration-<device>-<date>/backup.tar` on the same device (on the Mac also under
   `~/UCompute-migration/`), and verify the archive lists every inventoried file. Nothing is deleted.
4. Uninstall or rename the old app, unit and tasks only after the backup is verified.

## 3. Carrying data over (one-time, explicit)

- **Models and engines:** copied into the new data folder's store and kept only where the blob's SHA-256 matches its
  manifest; anything that does not match is re-downloaded, never trusted.
- **Settings** (privacy mode, networking, groups): re-applied on the new install from the inventory, then read back.
- **Device identity and pairings: intentional reset (default).** The renamed protocol cannot reach old peers anyway,
  so each device starts with a fresh identity and pairs again once; the old identity stays in the backup. Choosing to
  import the identity instead is possible (its key files are format-compatible) and would be recorded per device.
- **Audit and execution records, logs:** kept in the backup only (they describe the old installation).

## 4. Acceptance test per machine (condition 2)

Each item gives PASS/FAIL with evidence in `docs/evidence/migration-<device>-<date>/`:

1. Fresh install of the renamed build on a clean data folder: healthy, golden local inference.
2. Replacement of the previous development install: the old app, daemon, unit or tasks gone; no orphaned process;
   the new one healthy.
3. Startup after a reboot (owner-assisted where a reboot needs the machine's user).
4. Inference: local, remote (Mode B) to each other renamed device, split (configuration P between the Mac and
   sentinel; SINGLE-HOST elsewhere).
5. Mesh between updated Mac, Linux and Windows: discovery, fresh pairing (including a refused wrong code), revocation.
6. Outdated build: an old-build device dialling or being dialled by a renamed one fails at the TLS handshake with a
   readable error (no shared ALPN), and is never treated as a paired peer.
7. Failed installation: an interrupted or refused install leaves the previous working state usable, or the device can
   be restored from its backup.

The migration counts as done only when every reachable machine passes; an unreachable machine stays listed as pending.

## 5. Protocol identifiers (condition 3) and what is kept

Frozen interface names, changed only with a protocol version recorded in ADR-0003, never because the product's name
changes: ALPNs `ucompute/1` and `ucompute-pair/1`; pairing payload `ucompute-pair:v1:`; pairing key-export label
`ucompute-pair/1 confirm`; signature contexts `ucompute/1 capabilities\0` and `ucompute-revocation/1` (the signed mDNS
advert's canonical form starts `v=1`); mDNS service `_ucompute._udp`; protobuf package `ucompute.v1`.

Only support for the former names goes. Discovery, encrypted transport, secure pairing, revocation, persistent configuration
and storage all stay, under the new names, and so does versioning between UCompute releases: the major version is in
the ALPN, compatible minors negotiate `min(minor)`, and an incompatible peer gets a typed `protocol_incompatible` or
`peer_unauthorized` with an update hint. The rename must not weaken verification: every signature keeps its own
context, and tests refuse any other ALPN and any other `<name>-pair:` payload, the former ones included (`tls.rs`,
`pairing.rs` in `crates/ucompute-network`). An upgrade and compatibility strategy between UCompute versions is decided
before broad public adoption. This procedure is one-time and manual; no permanent migration code from the former names is
built.
