# Evidence folder

Names: the raw evidence files listed below (JSON, logs, text dumps, compressed logs) use the current names (D-030). What they describe ran under the former name; the original text is in git history (commit cab8a09 and earlier). Only names and identifiers were rewritten; every number, hash, size, date, time, count, version, verdict and commit id is unchanged. A recorded SHA-256 or size of a rewritten file still describes the original file, not the rewritten copy. Compressed files were decompressed, rewritten and recompressed with `gzip -n`.

Raw files rewritten under this note:

- `benchmarks/results/20261014T163802Z-stories15M-q4_0-cpu.json`
- `benchmarks/results/20261014T163828Z-stories15M-q4_0-gpu.json`
- `benchmarks/results/20261014T163910Z-qwen2.5-0.5b-instruct-q4_k_m-cpu.json`
- `benchmarks/results/20261014T163938Z-qwen2.5-0.5b-instruct-q4_k_m-gpu.json`
- `benchmarks/results/20261014T164004Z-smollm2-360m-instruct-q8_0-gpu.json`
- `benchmarks/results/20261014T164029Z-smollm2-360m-instruct-q8_0-cpu.json`
- `docs/evidence/desktop-packaged-mac-m2-3379fe9.json`
- `docs/evidence/desktop-packaged-mac-m2-ffe1a47.json`
- `docs/evidence/desktop-packaged-mac-m2-round7.json`
- `docs/evidence/desktop-ui2-packaged-mac-m2.json`
- `docs/evidence/linux-install-2026-10-24/verify-linux-install-0b0c1a4.json`
- `docs/evidence/linux-install-2026-10-24/verify-linux-install-lint.json`
- `docs/evidence/linux-install-2026-10-24/verify-linux-install-notices.json`
- `docs/evidence/linux-install-2026-10-24/verify-linux-install.json`
- `docs/evidence/linux-soak-2026-10-24/version.txt`
- `docs/evidence/linux-tests-2026-10-23/cargo-test-c811a90.log.gz`
- `docs/evidence/mesh-physical-2026-10-21/packaged-app-kyle-m2pro.json`
- `docs/evidence/mesh-physical-2026-10-21/run1-console.txt`
- `docs/evidence/mesh-physical-2026-10-21/run1-raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-21/run2-console.txt`
- `docs/evidence/mesh-physical-2026-10-21/run2-raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-21/run2-results.json`
- `docs/evidence/mesh-physical-2026-10-22-baseline/discovery/console.txt`
- `docs/evidence/mesh-physical-2026-10-22-baseline/discovery/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-22-baseline/discovery/results.json`
- `docs/evidence/mesh-physical-2026-10-22-baseline/hold/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-22-baseline/mesh/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-22-baseline/runner.log`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1831/discovery/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1831/discovery/results.json`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1831/mesh/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1831/runner.log`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1838/discovery/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1838/discovery/results.json`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1838/mesh/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1838/runner.log`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1845/discovery/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1845/discovery/results.json`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1845/hold/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1845/mesh/raw.jsonl.gz`
- `docs/evidence/mesh-physical-2026-10-23-linux/run-1845/runner.log`
- `docs/evidence/release-gate-3379fe9/cargo-test.log.gz`
- `docs/evidence/release-gate-3379fe9/clippy-windows-gnu.txt`
- `docs/evidence/release-gate-3379fe9/summary.txt`
- `docs/evidence/windows-app-2026-10-29/verify-77262a5.json`

Screenshots: in `docs/evidence/img/windows-app-chat-ran-on-mac-2026-10-19.png` and `docs/evidence/img/windows-app-wrong-pairing-code-2026-10-19.png` the app's name label at the top of the sidebar (pixels x 44-127, y 16-47) was painted over with the sidebar's white (D-030); no other pixel changed. The originals are in git history. The screenshots in `docs/evidence/img/desktop-ui2/` already showed UCompute and are unchanged.

Not rewritten: eight lines (71, 72, 202, 203, 333, 334, 464, 465) of `docs/evidence/windows-app-2026-10-29/verify-77262a5.json` hold a .NET Framework method name, from a recorded PowerShell type dump, that contains the former name as an English word. It is a third-party identifier in recorded output (like llama.cpp's own identifiers in recorded engine logs), so it is kept as recorded.
