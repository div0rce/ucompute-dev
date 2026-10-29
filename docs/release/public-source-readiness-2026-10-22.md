# Public source readiness: div0rce/ucompute (2026-10-22)

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

**Scope:** what has to happen before the public repository `div0rce/ucompute` can show this work. This covers the source code only. Binary releases are a separate decision (section 3).

**Inputs:** three read-only audits run on 2026-10-22 against branch `integration/recover-codex-m1` at HEAD `1004b3b`:
1. Secrets and history
2. Licences and third-party content
3. Personal and sensitive data

This synthesis re-checked the following against the repository:
- The branch has 58 commits. Four of them are already public on `origin/main`, which is at `1ae8424` and contains only `LICENSE` and `PROJECT_UCOMPUTE_AUTONOMOUS_HANDOFF_V2.md`. The other 54 have never been pushed, and every commit is signed (`%G? = G`).
- The tree has 239 tracked files.
- `deny.toml` has no `CDLA-Permissive-2.0`.
- `README.md:76` says "bundled".
- `.gitignore` has no `.env*`, `*.pem` or `*.key` entries.
- `.github/workflows/ci.yml` has no `permissions:` block and runs on pushes to `main` and `integration/**`.
- D-008, B-007 and D-020 all forbid pushing to this repository.

**This file reproduces no identifying values:** no usernames, addresses, emails, peer ids or keys. It names only files and kinds of data, so it can ship with the snapshot or be dropped. The audits' exact matches live in that session's temporary scratchpad, not in the repository.

## 1. Verdict

**Not ready today. It becomes ready after one redaction pass, which can be done entirely on this Mac, and a written owner authorization.**

- **Security:** no credentials, tokens or private keys appear anywhere in the history, the tree or the other local refs. Nothing needs rotating.
- **Licences:** nothing blocks publishing the source. The repository is Apache-2.0 and contains no copied GPL or AGPL code. No model weights or engine binaries were ever committed.
- **What does block it:**
  - Privacy: two third parties and the owner's own contact and access details.
  - Governance: no push is authorized yet.

## 2. Blockers, ranked

### Must fix before any push

| # | Class | Blocker | Where (at 1004b3b) | Fix |
|---|---|---|---|---|
| G1 | Governance | Pushing is forbidden until the owner authorizes it in writing. D-020 explicitly excludes "any push of div0rce/ucompute". | `docs/DECISIONS.md` D-008 and D-020, `docs/BLOCKERS.md` B-007, spec section 4 | Owner records a new D-entry with the exact scope of the push (section 5) and updates B-007 to match. |
| P1 | Privacy, third party | The owner of the second Mac (the M2 Pro). The tree contains that person's macOS username, their home LAN IP, the name of the SSH key file used for their Mac, their name, their machine's name and its real peer id. | Defaults at `scripts/mac-host.sh:10-16`. Everything in `docs/evidence/mesh-physical-2026-10-21.md` and `docs/evidence/mesh-physical-2026-10-21/`, including the packaged-app JSON (their name is in its file name, and their home path is at lines 48, 55 and 92). `docs/PROGRESS.md:9,88`. The commit message of `1004b3b`. | Make the script require environment variables with no defaults. Rename the person and their machine to "Mac B". Replace peer ids with placeholders and paths with `/Users/<user>/`. Rename the packaged-app JSON. Drop both `run*-raw.jsonl.gz` files: they hold about 500 matches inside gzip, out of reach of text replacement. Alternatively regenerate them redacted. |
| P2 | Privacy and security, third party | The Windows account on the HP laptop: its username, its `.local` hostname and `C:/Users/...` paths. The same document also describes the SSH server, the administrator-trusted key and the open firewall ports on that machine. Together that is a map for targeting it. The dev Mac's public key, which links the two machines, is there too. | `scripts/windows-host.sh:9-13`, `docs/environment/windows-test-host.md:18,62,67,68,84` | Use environment variables with no defaults, and placeholders (`<user>`, `<host>.local`, `<your public key>`). Keep the generic setup steps and drop the facts specific to this host. |
| P3 | Privacy, owner | The owner's two personal email addresses (which together give their full name and university), plus the live Cloudflare Access policy name, its login method, the protected hosts and a private chat quote | `docs/DECISIONS.md:35` (D-020); related private website details at `docs/VERIFICATION.md:260-266` | Reduce D-020 to its scope ("website infrastructure authorized by the owner on 2026-10-19; details private"). Generalize the VERIFICATION entry the same way. |

### Should fix in the same pass (not blocking)

| # | Class | Item | Fix |
|---|---|---|---|
| H1 | Privacy | **Real persistent peer ids** of the three machines (long and short forms). They appear in the mesh-physical files and the screenshots. | Replace them with placeholders, and crop the Mac's short label in `docs/evidence/img/windows-app-wrong-pairing-code-2026-10-19.png`. |
| | | **The owner's Mac hostname nickname** in `docs/evidence/m3-two-machine-2026-10-18.md:18` and the mesh-physical files. | Replace it with "Mac A". |
| | | **Home-folder, Codex chat-title and Claude scratchpad paths** (one of these includes a session id). They appear in: <br>• `docs/environment/ENVIRONMENT-Mac-2026-10-14T0848Z.md:159,169,244` <br>• `docs/PROGRESS.md:200` <br>• `docs/evidence/m3-single-host-2026-10-15.md:11,18,95,151` <br>• `docs/design/desktop-ui-2-plan.md:1039` <br>• `docs/design/ui2-research/api-inventory.md:3` <br>• the traceback in `run1-console.txt:11-20` | Generalize the paths. |
| | | **Hardware identifiers** in the ENVIRONMENT file: the battery id (line 85), the model number (line 40) and a `/var/folders` path (line 91). | Remove them. |
| | | **LAN and hotspot IPs** where they sit next to names. | Generalize them. |
| | | **The owner's surname in two test fixtures**: `apps/desktop/src-tauri/src/firewall.rs:237` and `crates/ucompute-network/src/pairing.rs:652`. | Change them after the UI 2.0 workflow finishes, since those folders are in use now. |
| H2 | Hygiene, CI | **The first push will turn CI red.** `cargo deny` rejects `webpki-roots` (licence CDLA-Permissive-2.0) under the current `deny.toml` allowlist. | Owner decision (section 5). |
| | | **The workflow has no `permissions:` block**, and its actions are pinned by tag rather than by SHA. | Add `permissions: contents: read`. |
| H3 | Accuracy | `README.md:76` calls llama.cpp "the bundled inference engine". It is not bundled: it is pinned (`catalog/backends.json`, b11609) and downloaded on first run, with its licence checked. | Change "bundled" to "downloaded". |
| H4 | Accuracy | `docs/decisions/ADR-0001-stack-and-license.md` still says dual MIT/Apache, with status Proposed. | Add "Superseded by D-009 (Apache-2.0)". |
| H5 | Licence hygiene | `LICENSE` names no copyright holder, and Cargo has no `authors`. | Add a root `NOTICE` containing a copyright line. |
| H6 | Hygiene | `.gitignore` lacks `.env*`, `*.pem` and `*.key`. `.claude/worktrees/` is ignored only through the local `.git/info/exclude`. | Add those patterns to `.gitignore`. |
| H7 | Security process | `SECURITY.md` points reporters to a private GitHub security advisory. That works only if private vulnerability reporting is turned on for the repository, and `SECURITY.md:28` itself asks for the route to be tested before external distribution. | The owner turns the setting on (it is a repository setting) and sends a harmless test report. |

These items are fine and can stay:
- Commit author: "nasr" with the GitHub noreply address.
- The `div0rce` handle.
- The "Codex (GPT-6)" provenance notes, once their paths are generalized.
- The fake test secrets in `crates/ucompute-observability` and `crates/ucompute-runtime`.
- The test IPs in code and fixtures.
- The expired one-time pairing codes.
- The app-UI screenshots, after cropping the label as in H1.
- The GPL-3.0 template `LICENSE` in the public commit `f2c2883`. It was replaced by Apache-2.0 before any code existed.

## 3. Blockers for binary publication only (not for this push)

These four items block publishing a dmg or msi. They do not affect the source push.

1. **No third-party notices ship.** `UCompute.app` contains only the two binaries, the icon and `Info.plist`, and the JS bundle has had its licence comments stripped.
   - About 550 statically linked crates require their licence texts to ship. Three need specific handling:
     - `ring` (Apache-2.0 AND ISC, with BoringSSL code)
     - `webpki-roots` (CDLA-Permissive-2.0)
     - the MPL-2.0 crates (`cssparser`, `cssparser-macros`, `selectors`, `dtoa-short`, `option-ext`)
   - Fix: generate a `THIRD_PARTY_NOTICES` file (cargo-about plus a JS licence list) and ship it through `bundle.resources`.
2. **The CI licence gate fails** (H2). The 167 platform-only crates that were not looked up stay unverified until `cargo deny` runs.
3. **The app icons are Tauri's default logo** (all 16 files in `apps/desktop/src-tauri/icons/`). Replace them with the original glyph planned in `docs/design/desktop-ui-2-plan.md`.
4. **Fixture models are visible to users.** The TinyStories fixture models (licence `unknown`) appear in the user catalog. Hide `fixture`-tagged entries in release builds.
   - Separately, the catalog's `license_url` entries point at `blob/main` rather than at pinned revisions.

## 4. Publication method

The audits disagreed on four points. This report resolves them as follows:
- **The owner's emails:** audit 1 rated them Low. Audit 3 rated them must-remove, and this report agrees. D-020 keeps the preview site private, and publishing the allowlist together with the policy shape helps anyone targeting it.
- **`git push --all`:** audit 1 called it acceptable. It is not. It would also push local `main` and a local feature branch whose name carries the hostname nickname. Under the recommended method below, `integration/recover-codex-m1` must never be pushed either. Use one explicit refspec.
- **filter-repo versus a snapshot:** audit 1 suggested filter-repo, audit 3 a snapshot. This report recommends the snapshot.
- **Is every sensitive string in history also in the current tree?** Audit 3 said yes. The exception is the three `.pyc` files with local paths that were removed in `66d2ba6`. That does not matter for a snapshot. It does matter for filter-repo.

| Option | How | For | Against |
|---|---|---|---|
| **A. Fresh reviewed snapshot (recommended)** | Build one signed commit whose tree is the redacted tree and whose parent is `origin/main` (`1ae8424`). Push it to `main` with an ordinary push. | Only one tree needs review. The `.gz` problem disappears. The 54 commit messages need no scrubbing. No force push, and the 4 public commits are kept. The full private history stays private. | The public history has no granularity. Commit hashes cited in `REQUIREMENTS.csv` (VERIFIED rows require one), evidence and PROGRESS do not resolve publicly. Private and public histories diverge unless development moves onto the public line. |
| B. Rewrite history with filter-repo | Run `git filter-repo` with `--replace-text`, `--replace-message`, and `--invert-paths` for the `.gz` and `.pyc` files, then push the branch. | Keeps the per-commit history. The commit map allows citations to be rewritten. | Every blob of all 54 commits must be verified, not just one tree, and a string missed in any intermediate commit is public permanently. The `.gz` files have to be dropped from all of history. 54 signed commits get rewritten and must be re-signed. Hashes change anyway. |
| C. Push as is | | | Rejected. It publishes P1 to P3. |

For option A, add a short note to the snapshot, for example in `README.md` or this folder. It should say that commit hashes cited in evidence before the public snapshot refer to unpublished development history, which the owner keeps privately.

**Command sequence, not run.** It assumes the redacted tree is committed on a local branch `publish-prep`:

```sh
git fetch origin                                            # origin/main must still be 1ae8424
git verify-commit publish-prep                              # optional; local branch
C=$(git commit-tree -S 'publish-prep^{tree}' -p origin/main -F <message-file>)
git rev-list --count origin/main..$C                        # must print 1
git diff --name-status origin/main $C                       # review the full file list
git push origin $C:refs/heads/main                          # the only push; no --all/--mirror/--tags
```

The push starts CI: a macOS-14 runner, the pinned fixture download from Hugging Face, and `cargo deny`. Fix H2 first, or accept a red first run.

**After publishing**, start new local work from the public commit, so that later pushes are ordinary ones that each pass a pre-push scan. Keep the old branch as an offline `git bundle` archive. If development stays on the old line instead, every public update has to be another hand-built snapshot.

## 5. Owner decisions needed

1. **Authorize the push in writing** in `docs/DECISIONS.md`. Spec section 4 and D-008 require this. The authorization should give its scope: one reviewed snapshot commit to `div0rce/ucompute` `main`, and nothing else. Update B-007 to match. Under option A, this record goes into the snapshot.
2. **Choose the method:** A (recommended) or B.
3. **Third parties: redact, or get their consent.** Redacting needs no contact with either person and can be done now. Keeping either name requires that person's explicit consent first.
4. **The owner's own data:**
   - Remove the emails (recommended).
   - Keep the AI-agent provenance notes (recommended).
   - Decide whether the business hypotheses go public: spec §26.3, §29.3–29.4, and the $10 / 15% example near spec line 545.
5. **The `.jsonl.gz` evidence:** drop it (simplest), or regenerate it redacted and keep the originals privately.
6. **CI licence gate:** either allow `CDLA-Permissive-2.0` in `deny.toml` or switch reqwest to platform roots. Also decide whether to fix this before the first push.
7. **Copyright holder name** for `NOTICE`.
8. **Repository settings, done by the owner:** turn on private vulnerability reporting and test it (H7).
9. **Clean-up after testing:**
   - Remove the dev Mac's key from the laptop's `authorized_keys` once testing ends.
   - Regenerate device identities only if real peer ids stay public. Redacting them avoids this, which would otherwise need both other machines.
10. **Website:** whether ucompute.org may link to the repository. That needs its own authorization under spec §29.5 and B-007.

## 6. Pre-push checklist

**Before you start**
- [ ] The UI 2.0 workflow has committed. The working tree now has 24 uncommitted or untracked paths in `apps/desktop/`, `crates/` and `docs/capabilities/`, and the audits did not cover them.
- [ ] The snapshot is built in a separate worktree from that committed tip, with `git status` clean.

**Authorization**
- [ ] The new D-entry authorizing this push, and the updated B-007, are in the snapshot tree.

**Redaction**
- [ ] P1, P2 and P3 are fixed.
- [ ] The H1 items are fixed, or each one is accepted by the owner.
- [ ] The `.gz` files are dropped or regenerated redacted.
- [ ] The screenshot label is cropped.

**Scanning**
- [ ] A denylist of exact strings is kept **outside the repository**. It covers:
  - both third-party usernames and names
  - both third-party hostnames
  - the owner's emails
  - the owner's hostname nickname
  - all real peer ids, in long and short forms
  - the LAN and hotspot IPs tied to people
  - the SSH public key body
  - the battery id
  - the Access policy name
  - the private website repository name
  - the literals `/Users/`, `C:\Users` and `C:/Users`
- [ ] `git grep -nIiE -f <denylist> publish-prep` returns only intended placeholders.
- [ ] `git ls-tree -r --name-only publish-prep | grep -iE -f <denylist>` returns nothing. File names count too.
- [ ] `git ls-tree -r --name-only publish-prep | grep -iE '\.(gz|zip|pyc|gguf|dylib|dll|exe|key|pem)$'` returns nothing, or every remaining file has been decompressed or opened and scanned.
- [ ] gitleaks or trufflehog runs clean over the tree, if available. This is optional, and the audits found no secrets.
- [ ] The snapshot commit message contains no names. The commit is signed (`git verify-commit $C`).

**Push**
- [ ] After `git fetch origin`, `origin/main` is still `1ae8424`.
- [ ] `git rev-list --count origin/main..$C` prints `1`.
- [ ] `git diff --name-status origin/main $C` has been reviewed.
- [ ] Push exactly `git push origin $C:refs/heads/main`. Never use `--all`, `--mirror`, `--tags` or a `refs/*` refspec. Never push `integration/recover-codex-m1`, that feature branch, `refs/recovered/*` or `refs/codex/*`.

**After the push**
- [ ] The public page shows only the reviewed tree.
- [ ] The CI result is recorded.
- [ ] The push is recorded in `docs/VERIFICATION.md` and `docs/PROGRESS.md`.
- [ ] A private `git bundle` of the full history is stored offline.
- [ ] Local development is moved onto the public commit.
