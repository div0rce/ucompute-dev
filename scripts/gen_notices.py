#!/usr/bin/env python3
"""Write THIRD_PARTY_NOTICES for what a build ships, and refuse a licence outside deny.toml's allow list.

  python3 scripts/gen_notices.py --out THIRD_PARTY_NOTICES.txt [--target <triple>] [--crate common-cli ...] [--js]

Rust: every crate reached from the named workspace crates (default: common-cli) through normal dependencies on the
target (default: this host), with its licence expression and the licence/notice files its package ships. --js adds
the desktop app's production npm packages (pnpm) and the vendored Lucide icons. Identical licence texts are written
once. Exit 1 when an expression is not satisfied by deny.toml's `allow` (ADR-0001: no GPL family).
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FILES = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT|UNLICENSE)([-._].*)?$", re.I)


def allowed():
    with open(os.path.join(ROOT, "deny.toml")) as f:
        text = f.read()
    return set(re.findall(r'"([^"]+)"', re.search(r"^allow\s*=\s*\[(.*?)\]", text, re.S | re.M).group(1)))


def satisfied(expr, allow):
    """SPDX expression check: OR needs one side allowed, AND both; `X WITH Y` must be allowed as written."""
    toks = re.findall(r"\(|\)|[^\s()]+", expr.replace("/", " OR "))
    pos = 0

    def atom():
        nonlocal pos
        t = toks[pos]
        pos += 1
        if t == "(":
            v = disj()
            pos += 1  # ")"
            return v
        if pos < len(toks) and toks[pos] == "WITH":
            t = f"{t} WITH {toks[pos + 1]}"
            pos += 2
        return t in allow

    def conj():
        nonlocal pos
        v = atom()
        while pos < len(toks) and toks[pos] == "AND":
            pos += 1
            v = atom() and v
        return v

    def disj():
        nonlocal pos
        v = conj()
        while pos < len(toks) and toks[pos] == "OR":
            pos += 1
            v = conj() or v
        return v

    try:
        ok = disj()
    except IndexError:  # an operator with nothing after it
        return False
    return ok and pos == len(toks)  # anything left over ("MIT and GPL-3.0-only") is not understood: refuse


def tool(name):
    """The program's full path: on Windows pnpm is `pnpm.cmd`, which subprocess does not find by its bare name."""
    return shutil.which(name) or name


def rust(crates, target):
    cmd = [tool("cargo"), "metadata", "--format-version", "1", "--locked", "--filter-platform", target]
    m = json.loads(subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, check=True).stdout)
    pkgs = {p["id"]: p for p in m["packages"]}
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    todo = [i for i in m["workspace_members"] if pkgs[i]["name"] in crates]
    if len(todo) != len(crates):
        sys.exit(f"unknown crate in {crates}")
    seen = set()
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        todo += [d["pkg"] for d in nodes[i]["deps"] if any(k["kind"] is None for k in d["dep_kinds"])]
    out = []
    for i in sorted(seen, key=lambda i: (pkgs[i]["name"], pkgs[i]["version"])):
        p = pkgs[i]
        if p["source"] is None:  # a workspace crate: this project's own LICENSE
            continue
        d = os.path.dirname(p["manifest_path"])
        files = sorted(os.path.join(d, f) for f in os.listdir(d) if FILES.match(f))
        if p.get("license_file"):
            files.append(os.path.join(d, p["license_file"]))
        out.append((p["name"], p["version"], p.get("license") or "", p.get("repository") or "", sorted(set(files))))
    return out


def js():
    desk = os.path.join(ROOT, "apps", "desktop")
    r = subprocess.run([tool("pnpm"), "licenses", "list", "--prod", "--json"], cwd=desk, capture_output=True, text=True, check=True)
    out = []
    for lic, items in json.loads(r.stdout).items():
        for it in items:
            d = it["paths"][0]
            files = sorted(os.path.join(d, f) for f in os.listdir(d) if FILES.match(f))
            for v in it["versions"]:
                out.append((it["name"], v, lic, it.get("homepage") or "", files))
    out.append(("lucide-static (icons in apps/desktop/src/ui/icons.tsx)", "0.460.0", "ISC", "https://lucide.dev",
                [os.path.join(desk, "src", "ui", "icons", "LICENSE-lucide.txt")]))
    return sorted(out)


def main():
    host = re.search(r"host: (\S+)", subprocess.run(["rustc", "-vV"], capture_output=True, text=True).stdout).group(1)
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--out", required=True)
    ap.add_argument("--target", default=host)
    ap.add_argument("--crate", action="append", dest="crates")
    ap.add_argument("--js", action="store_true")
    a = ap.parse_args()
    entries = rust(a.crates or ["common-cli"], a.target) + (js() if a.js else [])
    allow = allowed()
    bad = [(n, v, lic) for n, v, lic, _, _ in entries if not lic or not satisfied(lic, allow)]
    if bad:
        for n, v, lic in bad:
            print(f"licence not allowed by deny.toml: {n} {v}: {lic or '(none declared)'}", file=sys.stderr)
        return 1
    texts, lines = {}, ["Third-party software in this build of UCompute (formerly Common).", ""]
    for n, v, lic, url, files in entries:
        refs = []
        for f in files:
            with open(f, errors="replace") as fh:
                t = fh.read().strip()
            refs.append(texts.setdefault(t, f"T{len(texts) + 1}"))
        lines.append(f"{n} {v} — {lic}" + (f" — {url}" if url else "")
                     + (f" — licence text {', '.join(refs)}" if refs else " — no licence file in the package"))
    lines += ["", "=" * 78]
    for t, ref in sorted(texts.items(), key=lambda kv: int(kv[1][1:])):
        lines += ["", f"[{ref}]", "", t, "", "-" * 78]
    with open(a.out, "w") as f:
        f.write("\n".join(lines) + "\n")
    print(f"{len(entries)} packages, {len(texts)} distinct licence texts -> {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
