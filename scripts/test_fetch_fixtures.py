"""Offline checks that scripts/fetch-fixtures.sh asks for the llama.cpp archive catalog/backends.json pins for the
platform it runs on. `uname` is faked and `curl` is a stub that records its arguments and fails, so nothing is
downloaded and nothing here is evidence of an engine running.
Run: python3 -m unittest discover -s scripts -p 'test_*.py' -v
"""
import hashlib
import io
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(REPO, "scripts", "fetch-fixtures.sh")
with open(os.path.join(REPO, "catalog", "backends.json")) as f:
    PINS = {b["platform"]: b for b in json.load(f)["backends"]}


def fetch(uname_s, uname_m):
    """Run the script as if on `uname -s`/`uname -m`; returns (exit code, stderr, each curl call's arguments)."""
    with tempfile.TemporaryDirectory() as d:
        bin_, log = os.path.join(d, "bin"), os.path.join(d, "curl.log")
        os.mkdir(bin_)
        for name, body in {
            "uname": f'case "$1" in -s) echo "{uname_s}" ;; -m) echo "{uname_m}" ;; *) exit 1 ;; esac',
            "curl": f'printf "%s\\t" "$@" >> "{log}"; echo >> "{log}"; exit 22',
        }.items():
            with open(os.path.join(bin_, name), "w") as f:
                f.write(f"#!/bin/sh\n{body}\n")
            os.chmod(os.path.join(bin_, name), 0o755)
        env = dict(os.environ, PATH=f"{bin_}:{os.environ['PATH']}", COMMON_HOME=os.path.join(d, "home"))
        p = subprocess.run(["bash", SCRIPT], env=env, capture_output=True, text=True, timeout=60)
        if not os.path.exists(log):
            return p.returncode, p.stderr, []
        with open(log) as f:
            return p.returncode, p.stderr, [line.rstrip("\t").split("\t") for line in f.read().splitlines()]


class FetchFixturesPlatformTests(unittest.TestCase):
    def assert_fetches(self, uname_s, uname_m, platform):
        code, err, calls = fetch(uname_s, uname_m)
        self.assertNotEqual(code, 0, err)  # the stub's failed download stops the script
        self.assertEqual(len(calls), 1, calls)
        args = calls[0]
        self.assertEqual(args[-1], PINS[platform]["url"], args)
        out = args[args.index("-o") + 1]
        self.assertTrue(out.endswith(f"/backends/llama.cpp/{PINS[platform]['archive']}.part"), args)

    def test_linux_x64_fetches_the_ubuntu_cpu_pin(self):
        self.assert_fetches("Linux", "x86_64", "linux-x64")

    def test_macos_arm64_still_fetches_its_pin(self):
        self.assert_fetches("Darwin", "arm64", "macos-arm64")

    def test_windows_zip_pin_is_refused_before_any_download(self):
        code, err, calls = fetch("MINGW64_NT-10.0-26100", "x86_64")
        self.assertNotEqual(code, 0)
        self.assertEqual(calls, [])
        self.assertIn("common setup --fixtures", err)

    def test_an_unpinned_platform_is_refused_before_any_download(self):
        code, err, calls = fetch("Linux", "aarch64")
        self.assertNotEqual(code, 0)
        self.assertEqual(calls, [])
        self.assertIn("no llama.cpp build for linux-arm64", err)


def synthetic_run(d, models, require=None, corrupt=False):
    """Run a copy of the script in a synthetic repo under `d` on Linux x86_64: catalog/backends.json pins a tiny tar.gz
    and catalog/models.json lists `models` (id, tags), each a few SYNTHETIC bytes (not a model). The curl stub serves
    every pinned URL and logs it; with `corrupt`, it serves other bytes for the engine archive than the pin hashes.
    Returns (completed process, COMMON_HOME, pin, {id: catalog entry}, served URLs)."""
    repo, bin_, serve = os.path.join(d, "repo"), os.path.join(d, "bin"), os.path.join(d, "serve")
    for path in (os.path.join(repo, "scripts"), os.path.join(repo, "catalog"), bin_, serve):
        os.makedirs(path)
    shutil.copy(SCRIPT, os.path.join(repo, "scripts"))
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as t:
        data = b"synthetic engine, not llama.cpp\n"
        info = tarfile.TarInfo("llama-bTEST/llama-server")
        info.size = len(data)
        t.addfile(info, io.BytesIO(data))
    with open(os.path.join(serve, "synthetic.tar.gz"), "wb") as f:
        f.write(buf.getvalue())
    pin = {"name": "llama.cpp", "platform": "linux-x64", "archive": "synthetic.tar.gz",
           "url": "https://example.invalid/synthetic.tar.gz", "sha256": hashlib.sha256(buf.getvalue()).hexdigest(),
           "directory": "llama-bTEST", "release": "bTEST"}
    if corrupt:
        with open(os.path.join(serve, "synthetic.tar.gz"), "wb") as f:
            f.write(buf.getvalue()[:-1] + b"x")
    entries = {}
    for model_id, tags in models:
        blob = f"synthetic bytes for {model_id}, not a model\n".encode()
        with open(os.path.join(serve, f"{model_id}.gguf"), "wb") as f:
            f.write(blob)
        entries[model_id] = {"id": model_id, "name": f"Synthetic {model_id}", "file_name": f"{model_id}.gguf",
                             "url": f"https://example.invalid/{model_id}.gguf", "sha256": hashlib.sha256(blob).hexdigest(),
                             "bytes": len(blob), "license": "Apache-2.0", "license_url": "https://example.invalid/license",
                             "source": "synthetic", "chat": True, "redistribution": "allowed", "tags": tags}
    with open(os.path.join(repo, "catalog", "backends.json"), "w") as f:
        json.dump({"backends": [pin]}, f)
    with open(os.path.join(repo, "catalog", "models.json"), "w") as f:
        json.dump({"schema_version": 1, "models": list(entries.values())}, f)
    log = os.path.join(d, "curl.log")
    for name, body in {
        "uname": 'case "$1" in -s) echo Linux ;; -m) echo x86_64 ;; *) exit 1 ;; esac',
        # Serve https://example.invalid/<name> from the serve directory; anything else fails.
        "curl": f'out=; prev=; for a in "$@"; do [ "$prev" = -o ] && out="$a"; prev="$a"; done\n'
                f'echo "$a" >> "{log}"\n'
                f'case "$a" in https://example.invalid/*) cp "{serve}/${{a##*/}}" "$out" && exit 0 ;; esac\nexit 22',
    }.items():
        with open(os.path.join(bin_, name), "w") as f:
            f.write(f"#!/bin/sh\n{body}\n")
        os.chmod(os.path.join(bin_, name), 0o755)
    home = os.path.join(d, "home")
    env = dict(os.environ, PATH=f"{bin_}:{os.environ['PATH']}", COMMON_HOME=home)
    env.pop("COMMON_REQUIRE_FIXTURES", None)
    if require is not None:
        env["COMMON_REQUIRE_FIXTURES"] = require
    p = subprocess.run(["bash", os.path.join(repo, "scripts", "fetch-fixtures.sh")], env=env,
                       capture_output=True, text=True, timeout=60)
    served = []
    if os.path.exists(log):
        with open(log) as f:
            served = f.read().split()
    return p, home, pin, entries, served


MODELS = [("tiny-a", ["fixture"]), ("big-b", ["chat", "test-suite"]), ("other-c", ["embedding"])]


class FetchFixturesArchiveTests(unittest.TestCase):
    def test_a_pinned_archive_is_kept_and_its_directory_extracted_as_the_release(self):
        """The pin's sha256, directory and release are each checked against what the script does with them (a swap of
        any two fails here)."""
        with tempfile.TemporaryDirectory() as d:
            p, home, _, _, _ = synthetic_run(d, MODELS)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertIn("verified synthetic.tar.gz", p.stdout, p.stderr)
            engines = os.path.join(home, "backends", "llama.cpp")
            self.assertTrue(os.path.isfile(os.path.join(engines, "synthetic.tar.gz")), p.stderr)
            self.assertTrue(os.path.isfile(os.path.join(engines, "bTEST", "llama-server")), p.stderr)


    def test_an_archive_with_the_wrong_hash_is_deleted_and_never_extracted(self):
        with tempfile.TemporaryDirectory() as d:
            p, home, _, _, _ = synthetic_run(d, MODELS, corrupt=True)
            self.assertNotEqual(p.returncode, 0)
            self.assertIn("sha256 mismatch", p.stderr)
            engines = os.path.join(home, "backends", "llama.cpp")
            self.assertFalse(os.path.exists(os.path.join(engines, "synthetic.tar.gz")), "a bad archive was kept")
            self.assertFalse(os.path.exists(os.path.join(engines, "bTEST")), "a bad archive was extracted")


class FetchFixturesModelTests(unittest.TestCase):
    def assert_provisioned(self, home, entry):
        blob = os.path.join(home, "models", "blobs", f"sha256-{entry['sha256']}")
        with open(blob, "rb") as f:
            self.assertEqual(hashlib.sha256(f.read()).hexdigest(), entry["sha256"])
        with open(os.path.join(home, "models", "manifests", f"{entry['id']}.json")) as f:
            m = json.load(f)
        # The fields `common models pull` writes (crates/common-artifacts download.rs manifest_from).
        for key, want in {"id": entry["id"], "name": entry["name"], "file_name": entry["file_name"],
                          "sha256": entry["sha256"], "bytes": entry["bytes"], "license": entry["license"],
                          "license_url": entry["license_url"], "source_url": entry["url"],
                          "redistribution": entry["redistribution"], "chat": entry["chat"],
                          "tags": entry["tags"]}.items():
            self.assertEqual(m[key], want, key)

    def test_fixture_tagged_models_only_by_default(self):
        with tempfile.TemporaryDirectory() as d:
            p, home, pin, entries, served = synthetic_run(d, MODELS)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(served, [pin["url"], entries["tiny-a"]["url"]])
            self.assert_provisioned(home, entries["tiny-a"])

    def test_require_fixtures_also_provisions_the_test_suite_models(self):
        with tempfile.TemporaryDirectory() as d:
            p, home, pin, entries, served = synthetic_run(d, MODELS, require="1")
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertEqual(served, [pin["url"], entries["tiny-a"]["url"], entries["big-b"]["url"]])
            for model_id in ("tiny-a", "big-b"):
                self.assert_provisioned(home, entries[model_id])

    def test_the_real_catalog_tags_every_model_the_strict_suite_loads(self):
        """The Rust side checks the test files against these tags (catalog.rs suite_fixtures_cover_every_model_the_tests_name)."""
        with open(os.path.join(REPO, "catalog", "models.json")) as f:
            tagged = {m["id"] for m in json.load(f)["models"] if {"fixture", "test-suite"} & set(m.get("tags", []))}
        self.assertTrue({"stories15M-q4_0", "stories260K", "qwen2.5-0.5b-instruct-q4_k_m",
                         "smollm2-360m-instruct-q8_0", "qwen2.5-1.5b-instruct-q4_k_m"} <= tagged, tagged)


if __name__ == "__main__":
    unittest.main()
