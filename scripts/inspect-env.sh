#!/usr/bin/env bash
# Phase 1 environment inspection for UCompute (spec section 16 step 1).
#
# Read-only. Writes one Markdown report under docs/environment/ and prints its path.
# Never prints secrets: no env dumps, no tokens, git email masked.
#
# Usage: scripts/inspect-env.sh [output_dir]
set -u
export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-$ROOT/docs/environment}"
HOST="$(hostname -s 2>/dev/null || hostname)"
STAMP="$(date -u +%Y-%m-%dT%H%MZ)"
OUT="$OUT_DIR/ENVIRONMENT-${HOST}-${STAMP}.md"
OS="$(uname -s)"
mkdir -p "$OUT_DIR"

# Portable timeout (macOS has no coreutils timeout by default; perl ships with macOS).
tmo() { local s="$1"; shift; perl -e 'alarm shift; exec @ARGV or exit 127' "$s" "$@"; }
have() { command -v "$1" >/dev/null 2>&1; }

{
  echo "# Environment — ${HOST}"
  echo
  echo "- Captured (UTC): ${STAMP}"
  echo "- Script: scripts/inspect-env.sh (read-only)"
  echo "- OS kernel: $(uname -srm)"
  echo
} >"$OUT"

section() { printf '\n## %s\n\n' "$1" >>"$OUT"; }
# block "<label>" cmd args... — run with 30 s timeout, capture stdout+stderr in a code block.
block() {
  local label="$1"; shift
  printf '**%s** — `%s`\n\n```\n' "$label" "$*" >>"$OUT"
  if have "$1"; then
    tmo 30 "$@" >>"$OUT" 2>&1 || echo "[exit $?]" >>"$OUT"
  else
    echo "[not installed: $1]" >>"$OUT"
  fi
  printf '```\n\n' >>"$OUT"
}
kv() { printf '| %s | %s |\n' "$1" "$2" >>"$OUT"; }
ver() { have "$1" && { tmo 15 "$@" 2>&1 | head -n 1; } || echo "not installed"; }

# ------------------------------------------------------------------ summary
section "Summary"
echo "| Item | Value |" >>"$OUT"; echo "|---|---|" >>"$OUT"
if [ "$OS" = "Darwin" ]; then
  kv "Chip" "$(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
  kv "Model identifier" "$(sysctl -n hw.model 2>/dev/null)"
  kv "Unified memory (GiB)" "$(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1073741824 ))"
  kv "CPU cores (perf/eff/total)" "$(sysctl -n hw.perflevel0.physicalcpu 2>/dev/null)/$(sysctl -n hw.perflevel1.physicalcpu 2>/dev/null)/$(sysctl -n hw.physicalcpu 2>/dev/null)"
  kv "macOS" "$(sw_vers -productName 2>/dev/null) $(sw_vers -productVersion 2>/dev/null) ($(sw_vers -buildVersion 2>/dev/null))"
  kv "GPU cores" "$(system_profiler SPDisplaysDataType 2>/dev/null | awk -F': ' '/Total Number of Cores/{print $2; exit}')"
  kv "Metal support" "$(system_profiler SPDisplaysDataType 2>/dev/null | awk -F': ' '/Metal (Support|Family)/{print $2; exit}')"
else
  kv "CPU" "$(awk -F': ' '/model name/{print $2; exit}' /proc/cpuinfo 2>/dev/null)"
  kv "Memory (GiB)" "$(awk '/MemTotal/{printf "%.1f", $2/1048576}' /proc/meminfo 2>/dev/null)"
  kv "CPU threads" "$(nproc 2>/dev/null)"
  kv "OS" "$( (. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME") || uname -sr)"
fi
kv "Arch" "$(uname -m)"
kv "Free disk on repo volume" "$(df -h "$ROOT" 2>/dev/null | awk 'NR==2{print $4" free of "$2}')"
kv "git" "$(ver git --version)"
kv "rustc" "$(ver rustc -V)"
kv "cargo" "$(ver cargo -V)"
kv "cmake" "$(ver cmake --version)"
kv "clang" "$(ver clang --version)"
kv "python3" "$(ver python3 -V)"
kv "node / pnpm" "$(ver node -v) / $(ver pnpm -v)"

# ------------------------------------------------------------------ hardware
section "Hardware"
if [ "$OS" = "Darwin" ]; then
  block "Hardware overview" system_profiler SPHardwareDataType -detailLevel mini
  block "GPU / displays" system_profiler SPDisplaysDataType
  block "Memory pressure" memory_pressure -Q
  block "Power / battery" pmset -g batt
else
  block "CPU" lscpu
  block "Memory" free -h
  block "GPUs" lspci
fi

# ------------------------------------------------------------------ Metal device capabilities
if [ "$OS" = "Darwin" ]; then
  section "Metal device (MTLDevice)"
  SWIFT_BASE="$(mktemp -t ucompute_metal)"
  SWIFT_SRC="$SWIFT_BASE.swift"
  cat >"$SWIFT_SRC" <<'SWIFT'
import Metal
guard let d = MTLCreateSystemDefaultDevice() else { print("no Metal device"); exit(1) }
print("name: \(d.name)")
print("hasUnifiedMemory: \(d.hasUnifiedMemory)")
print("recommendedMaxWorkingSetSize_bytes: \(d.recommendedMaxWorkingSetSize)")
print("maxBufferLength_bytes: \(d.maxBufferLength)")
let fams: [(String, MTLGPUFamily)] = [("apple7", .apple7), ("apple8", .apple8), ("apple9", .apple9), ("metal3", .metal3)]
for (n, f) in fams { print("supportsFamily.\(n): \(d.supportsFamily(f))") }
SWIFT
  block "MTLDevice probe (swift)" swift "$SWIFT_SRC"
  rm -f "$SWIFT_SRC" "$SWIFT_BASE"
fi

# ------------------------------------------------------------------ toolchains
section "Compilers and toolchains"
if [ "$OS" = "Darwin" ]; then
  block "Xcode path" xcode-select -p
  block "Xcode version (full Xcode only)" xcodebuild -version
  block "Metal compiler" xcrun -f metal
  block "macOS SDK" xcrun --show-sdk-version
fi
block "clang" clang --version
block "cmake" cmake --version
block "ninja" ninja --version
block "rustup" rustup show
block "cargo-nextest" cargo nextest --version
block "go" go version
block "python3" python3 -V
block "node" node -v
block "pnpm" pnpm -v
block "Homebrew" brew --version

# ------------------------------------------------------------------ git
section "Git configuration"
{
  echo "| Setting | Value |"; echo "|---|---|"
  echo "| git version | $(ver git --version) |"
  echo "| user.name set | $(git config --global user.name >/dev/null 2>&1 && echo yes || echo no) |"
  em="$(git config --global user.email 2>/dev/null)"
  echo "| user.email | $( [ -n "$em" ] && echo "set (${em%%@*}@…)" || echo "not set") |"
  echo "| init.defaultBranch | $(git config --global init.defaultBranch 2>/dev/null || echo unset) |"
  echo "| commit.gpgsign | $(git config --global commit.gpgsign 2>/dev/null || echo unset) |"
  echo "| gh CLI logged in | $(have gh && (gh auth status >/dev/null 2>&1 && echo yes || echo no) || echo 'gh not installed') |"
} >>"$OUT"

# ------------------------------------------------------------------ network
section "Network reachability"
echo "| Endpoint | HTTP status / seconds |" >>"$OUT"; echo "|---|---|" >>"$OUT"
for u in https://github.com https://huggingface.co https://crates.io https://static.rust-lang.org https://registry.npmjs.org https://pypi.org; do
  r="$(have curl && tmo 12 curl -sS -o /dev/null -w '%{http_code} / %{time_total}' --max-time 10 "$u" 2>&1 || echo unreachable)"
  kv "$u" "$r"
done
if [ "$OS" = "Darwin" ]; then
  echo >>"$OUT"
  echo "Default route interface: \`$(route -n get default 2>/dev/null | awk '/interface/{print $2}')\`" >>"$OUT"
fi

# ------------------------------------------------------------------ model files
section "Existing model files"
{
  echo '```'
  if [ "$OS" = "Darwin" ] && have mdfind; then
    tmo 30 mdfind 'kMDItemFSName == "*.gguf"c' 2>/dev/null | head -n 200 | while IFS= read -r f; do
      printf '%12s  %s\n' "$(stat -f %z "$f" 2>/dev/null)" "$f"
    done
  else
    tmo 60 find "$HOME" -xdev -name '*.gguf' -size +1k 2>/dev/null | head -n 200 | while IFS= read -r f; do
      printf '%12s  %s\n' "$(stat -c %s "$f" 2>/dev/null)" "$f"
    done
  fi
  echo '```'
  echo
  echo "| Known model store | Size |"; echo "|---|---|"
  for d in "$HOME/.ollama/models" "$HOME/.cache/huggingface/hub" "$HOME/.cache/llama.cpp" "$HOME/Library/Caches/llama.cpp" "$HOME/.lmstudio/models" "$HOME/.cache/lm-studio/models"; do
    [ -d "$d" ] && echo "| $d | $(du -sh "$d" 2>/dev/null | cut -f1) |"
  done
} >>"$OUT"

# ------------------------------------------------------------------ runtimes
section "Available inference runtimes"
{
  echo "| Runtime | Status |"; echo "|---|---|"
  for b in llama-cli llama-server llama-bench rpc-server ollama lms exo whisper-cli; do
    echo "| $b | $(have "$b" && echo "$(command -v "$b")" || echo 'not on PATH') |"
  done
  echo "| ollama version | $(ver ollama --version) |"
  echo "| python mlx | $(have python3 && (tmo 20 python3 -c 'import mlx.core as m; print(m.__version__)' 2>/dev/null || echo 'not importable') || echo 'no python3') |"
  echo "| brew llama.cpp | $(have brew && (brew list --versions llama.cpp 2>/dev/null || echo 'not installed') || echo 'no brew') |"
} >>"$OUT"

echo "$OUT"
