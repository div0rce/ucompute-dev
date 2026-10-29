# GitHub: audit and proposal for private source synchronization (2026-10-29) — FOR OWNER APPROVAL

The owner asked (2026-10-29, engineering session) to replace local-only development with GitHub synchronization:
private sync of committed source, feature branches, automated CI and reviewable pull requests, no history rewriting,
without pushing uncommitted work or overwriting the existing public repository, and with private source sync, public
open-source publication and downloadable releases authorized separately. Nothing below has been done: AGENTS.md and
spec section 4 forbid any push or new remote until the owner authorizes it in writing in `docs/DECISIONS.md`.

## 1. Audit (read-only, at 4cbd1ac)

| Item | Finding |
|---|---|
| Remotes | One: `origin` = `https://github.com/div0rce/common.git` |
| `div0rce/common` on GitHub | **PUBLIC**, created 2026-10-14, last push 2026-10-14, 3 commits (`f2c2883` initial, `d5a593a` merge, `1ae8424` Apache-2.0 licence), branch `main` only. Nothing since has been pushed |
| Local branches | `integration/recover-codex-m1` (canonical, 163 commits, HEAD 4cbd1ac); `main` and `mous/epic-bhabha-fcf66a` at 1ae8424 (the latter is the website session's worktree branch) |
| Ancestry | `1ae8424` is an ancestor of HEAD: a new repository's `main` can start from HEAD with no rewrite |
| Uncommitted work | None (round 8 is committed: dc6bfa3, 9dd5d27, fbc0e67) |
| Size | 4.8 MiB pack; no blob over 1 MB; models and engines are never committed (`.common/` is ignored) |
| Secrets | `gitleaks git` over the whole history: no leaks (four synthetic test fixtures recorded in `.gitleaksignore`, 21d3dd4) |
| Dependency licences and advisories | `cargo deny check licenses sources advisories`: ok (TLS advisories fixed in b555109; four Linux-desktop-only unmaintained notices recorded in `deny.toml`) |
| Personal and third-party data | In the tree and the history: Kyle's macOS username (3 files, 2 commits), Jess's Windows account name (5 files, 5 commits), the laptop's host name (19 files), home-LAN IP addresses (52 files, 27 commits), the owner's two email addresses (one older decisions entry), sentinel's SSH user. Acceptable in a private repository; **blocks public publication** of this history |
| Licence and notices | `LICENSE` is Apache-2.0 but names no copyright holder; no `NOTICE` file (public-source readiness H5). Bundles ship generated THIRD_PARTY_NOTICES |
| GitHub account | `div0rce`, authenticated with `repo` scope. `div0rce/ucompute` does not exist (name free). Other repositories: `ucompute-website` (private), `sentinel` (public, an unrelated project). The Core Lab repository has no remote |
| CI | `.github/workflows/ci.yml` exists (Gate A and B jobs) and has never run |

## 2. Three actions, each authorized separately

1. **Private source synchronization** (proposed now, §3): committed source to a private repository visible only to
   the owner and invited collaborators.
2. **Public open-source publication** (not proposed now, §4): any public repository, mirror or branch.
3. **Downloadable releases** (unchanged): every artifact needs its own promotion record and the owner's approval of
   its exact SHA-256 (as D-025), and a public download also needs Gate G (signing, notarization, B-008).

Approving one never implies another.

## 3. Proposal: private synchronization

| Item | Proposed |
|---|---|
| Repository | `github.com/div0rce/ucompute`, **private**, created empty (`gh repo create div0rce/ucompute --private`), no template, no auto-generated files |
| Existing public `div0rce/common` | **Left untouched**: no push, no rename, no deletion. Locally its remote is renamed `public-common` with pushing disabled (`git remote set-url --push public-common DISABLED`), so no command can reach it by accident |
| New `origin` | `git@github.com:div0rce/ucompute.git` |
| First push | `git push origin <HEAD at approval>:refs/heads/main`, the full history of `integration/recover-codex-m1` (all commits, as they are). No other branch, no tags. The website worktree branch is not pushed |
| Branch model | `main` is the integration branch. Work happens on feature branches (`fix/…`, `feat/…`, `wp04/…`), each merged by pull request after CI passes. No force-push, no history rewriting, ever; merges are merge commits or squash merges, never rewrites of `main` |
| Protection | Branch protection and rulesets on a private repository need a paid GitHub plan, which the hard rules exclude (no paid services). Instead: a committed pre-push hook refusing direct pushes to `main` and any force-push, plus the rule in AGENTS.md. If the owner later chooses a paid plan, server-side protection replaces the hook |
| CI | On every pull request and push to `main`: registry validation, `cargo fmt`/`clippy`/`test` without fixtures (ubuntu-latest), the Python offline suites, desktop vitest and tsc, `cargo deny`, `gitleaks`. macOS and real-inference jobs run on demand (manual dispatch) to stay inside the free Actions minutes; no repository secrets; the account's spending limit stays $0, so jobs stop rather than bill |
| Who pushes | Engineering sessions, on feature branches; merges after CI and review |
| Hosts | sentinel and the Windows laptop keep receiving source by `scripts/mac-host.sh sync` / `windows-host.sh sync` from the Mac (no GitHub credentials on test hosts) |
| Policy change | AGENTS.md's "never push, never create a remote" becomes: push only to the private `origin` per the recorded decision; the public repository, publication and releases stay separately gated |

What the owner would approve, in one line: *"I authorize private synchronization to a new private repository
div0rce/ucompute as proposed in docs/release/github-sync-proposal-2026-10-29.md; div0rce/common stays untouched."*

## 4. Risks of public publication (for a later, separate decision)

- **History holds personal and third-party data** (§1). Publishing this history would expose Kyle's and Jess's
  account names, the owner's home network addresses and emails. With history rewriting excluded, a public release
  would be a new repository seeded from one redacted snapshot commit, with this private repository keeping the history.
- **Copyright holder and NOTICE** are missing; naming and trademark clearance are open (D-021).
- **Security posture**: the stage server used for split inference is unsafe against a hostile head (ADR-0006
  Proposed); a public repository exposes the threat model and the open review findings
  (`docs/design/wp04-round8-open-findings.md`) before they are fixed.
- **Claims**: public text must follow the capability snapshot and the website's claim rules; the repository's README
  and docs would become public claims.
- **The existing public `div0rce/common`** shows an empty Apache-2.0 project; whether to archive it, rename it or reuse
  it as the public home is part of that decision.
