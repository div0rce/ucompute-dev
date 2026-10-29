//! UCOMPUTE_BUILD_COMMIT, served by /health so a two-machine run can check that both daemons were built from the
//! same commit (WP-04 P1 preflight, privacy:F4). In a git checkout: HEAD, with "-dirty" when `git status
//! --porcelain --untracked-files=no` lists anything. In a tree copied by `scripts/mac-host.sh sync` (no .git): the
//! commit that script records in `.ucompute-sync-head`. Otherwise "unknown".
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Option<String> {
    // GIT_OPTIONAL_LOCKS=0: `status` must not refresh the index, or every build would rerun this script.
    let out = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn watch(p: &Path) {
    if p.exists() {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let marker = root.join(".ucompute-sync-head");
    let commit = if root.join(".git").exists() {
        if let Some(dir) = git(&root, &["rev-parse", "--absolute-git-dir"]) {
            watch(&Path::new(&dir).join("HEAD"));
            watch(&Path::new(&dir).join("logs/HEAD")); // every commit and checkout appends to it
        }
        match (
            git(&root, &["rev-parse", "HEAD"]),
            // Untracked files are not part of the build, and the P1 preflight ignores them too.
            git(&root, &["status", "--porcelain", "--untracked-files=no"]),
        ) {
            (Some(head), Some(st)) if st.is_empty() => head,
            (Some(head), Some(_)) => format!("{head}-dirty"),
            _ => "unknown".into(),
        }
    } else {
        std::fs::read_to_string(&marker).map_or_else(|_| "unknown".into(), |s| s.trim().to_string())
    };
    // Rerun on any source edit so "-dirty" follows the tree the binary is built from.
    for p in ["crates", "catalog", "Cargo.toml", "Cargo.lock"] {
        watch(&root.join(p));
    }
    watch(&marker);
    println!("cargo:rustc-env=UCOMPUTE_BUILD_COMMIT={commit}");
}
