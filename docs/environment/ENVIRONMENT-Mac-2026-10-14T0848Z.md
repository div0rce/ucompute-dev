# Environment — Mac

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

- Captured (UTC): 2026-10-14T0848Z
- Script: scripts/inspect-env.sh (read-only)
- OS kernel: Darwin 25.2.0 arm64


## Summary

| Item | Value |
|---|---|
| Chip | Apple M2 |
| Model identifier | Mac14,15 |
| Unified memory (GiB) | 8 |
| CPU cores (perf/eff/total) | 4/4/8 |
| macOS | macOS 26.2 (25C5048a) |
| GPU cores | 10 |
| Metal support | Metal 4 |
| Arch | arm64 |
| Free disk on repo volume | 14Gi free of 228Gi |
| git | git version 2.56.0 |
| rustc | rustc 1.98.0 (88d9e12ae 2026-08-18) |
| cargo | cargo 1.98.0 (797e8a9bc 2026-08-05) |
| cmake | cmake version 4.4.4 |
| clang | Apple clang version 21.0.0 (clang-2100.1.1.101) |
| python3 | Python 3.14.8 |
| node / pnpm | v26.10.0 / 10.34.4 |

## Hardware

**Hardware overview** — `system_profiler SPHardwareDataType -detailLevel mini`

```
Hardware:

    Hardware Overview:

      Model Name: MacBook Air
      Model Identifier: Mac14,15
      Model Number: MQKW3LL/A
      Chip: Apple M2
      Total Number of Cores: 8 (4 performance and 4 efficiency)
      Memory: 8 GB
      System Firmware Version: 13822.61.10
      OS Loader Version: 13822.61.10

```

**GPU / displays** — `system_profiler SPDisplaysDataType`

```
Graphics/Displays:

    Apple M2:

      Chipset Model: Apple M2
      Type: GPU
      Bus: Built-In
      Total Number of Cores: 10
      Vendor: Apple (0x106b)
      Metal Support: Metal 4
      Displays:
        Color LCD:
          Display Type: Built-in Liquid Retina Display
          Resolution: 2880 x 1864 Retina
          Main Display: Yes
          Mirror: Off
          Online: Yes
          Automatically Adjust Brightness: Yes
          Connection Type: Internal

```

**Memory pressure** — `memory_pressure -Q`

```
The system has 8589934592 (524288 pages with a page size of 16384).
System-wide memory free percentage: 38%
```

**Power / battery** — `pmset -g batt`

```
Now drawing from 'AC Power'
 -InternalBattery-0 (id=22675555)	83%; charging; 0:56 remaining present: true
```


## Metal device (MTLDevice)

**MTLDevice probe (swift)** — `swift /var/folders/kd/y0p61xc13zn_kwdpg38kw9h00000gn/T/ucompute_metal.ENnnU8coyF.swift`

```
name: Apple M2
hasUnifiedMemory: true
recommendedMaxWorkingSetSize_bytes: 5726633984
maxBufferLength_bytes: 4294967296
supportsFamily.apple7: true
supportsFamily.apple8: true
supportsFamily.apple9: false
supportsFamily.metal3: true
```


## Compilers and toolchains

**Xcode path** — `xcode-select -p`

```
/Applications/Xcode.app/Contents/Developer
```

**Xcode version (full Xcode only)** — `xcodebuild -version`

```
Xcode 26.6
Build version 17F113
```

**Metal compiler** — `xcrun -f metal`

```
/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/metal
```

**macOS SDK** — `xcrun --show-sdk-version`

```
26.5
```

**clang** — `clang --version`

```
Apple clang version 21.0.0 (clang-2100.1.1.101)
Target: arm64-apple-darwin25.2.0
Thread model: posix
InstalledDir: /Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin
```

**cmake** — `cmake --version`

```
cmake version 4.4.4

CMake suite maintained and supported by Kitware (kitware.com/cmake).
```

**ninja** — `ninja --version`

```
1.13.2
```

**rustup** — `rustup show`

```
Default host: aarch64-apple-darwin
rustup home:  /Users/nasr/.rustup

installed toolchains
--------------------
stable-aarch64-apple-darwin (default)
1.98.0-aarch64-apple-darwin (active)

active toolchain
----------------
name: 1.98.0-aarch64-apple-darwin
active because: overridden by '/Users/nasr/repos/ucompute/rust-toolchain.toml'
installed targets:
  aarch64-apple-darwin
```

**cargo-nextest** — `cargo nextest --version`

```
error: no such command: `nextest`

help: a command with a similar name exists: `test`

help: view all installed commands with `cargo --list`
help: find a package to install `nextest` with `cargo search cargo-nextest`
[exit 101]
```

**go** — `go version`

```
[not installed: go]
```

**python3** — `python3 -V`

```
Python 3.14.8
```

**node** — `node -v`

```
v26.10.0
```

**pnpm** — `pnpm -v`

```
10.34.4
```

**Homebrew** — `brew --version`

```
Homebrew 7.0.8
```


## Git configuration

| Setting | Value |
|---|---|
| git version | git version 2.56.0 |
| user.name set | yes |
| user.email | set (156965421+div0rce@…) |
| init.defaultBranch | main |
| commit.gpgsign | true |
| gh CLI logged in | yes |

## Network reachability

| Endpoint | HTTP status / seconds |
|---|---|
| https://github.com | 200 / 0.163200 |
| https://huggingface.co | 200 / 0.085063 |
| https://crates.io | 403 / 0.057361 |
| https://static.rust-lang.org | 200 / 0.103670 |
| https://registry.npmjs.org | 200 / 0.094036 |
| https://pypi.org | 200 / 0.051183 |

Default route interface: `en0`

## Existing model files

```
    19077344  /Users/nasr/Documents/Codex/2026-10-14/referenced-chatgpt-conversation-this-is-an/outputs/ucompute/models/stories15M-q4_0.gguf
```

| Known model store | Size |
|---|---|

## Available inference runtimes

| Runtime | Status |
|---|---|
| llama-cli | not on PATH |
| llama-server | not on PATH |
| llama-bench | not on PATH |
| rpc-server | not on PATH |
| ollama | not on PATH |
| lms | not on PATH |
| exo | not on PATH |
| whisper-cli | not on PATH |
| ollama version | not installed |
| python mlx | not importable |
| brew llama.cpp | not installed |
