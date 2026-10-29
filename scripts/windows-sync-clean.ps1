# Delete files under -Root that HEAD does not track (the list in .sync-manifest, written by
# scripts/windows-host.sh sync), keeping build outputs: target/, .ucompute/, node_modules/, the staged
# sidecar, gen/ and dist/. Refuses to delete anything when the result looks implausible, so a path bug
# can never wipe the tree (2026-10-22: an escaping bug once computed every relative path wrongly).
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-sync-clean.ps1 -Root C:\Users\<user>\src\ucompute [-DryRun]
param(
    [Parameter(Mandatory = $true)][string]$Root,
    [int]$Max = 200,
    [switch]$DryRun
)
$ErrorActionPreference = "Stop"
$root = (Get-Item -LiteralPath $Root).FullName.TrimEnd('\')
# NTFS ignores case: a file extracted into a folder whose case changed keeps the old folder's case on disk,
# so a case-sensitive match would delete it. The manifest is UTF-8 (Windows PowerShell 5.1 defaults to ANSI).
$keep = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
Get-Content -Encoding UTF8 -LiteralPath (Join-Path $root '.sync-manifest') | ForEach-Object { [void]$keep.Add($_) }
if ($keep.Count -lt 50) { throw "the manifest lists only $($keep.Count) files; refusing to clean" }
$skip = '^(target|\.ucompute)/|(^|/)node_modules/|^apps/desktop/src-tauri/(binaries|gen|target)/|^apps/desktop/dist/'
$present = 0
$stale = @(Get-ChildItem -LiteralPath $root -Recurse -File -Force | ForEach-Object {
        $rel = $_.FullName.Substring($root.Length + 1).Replace('\', '/')
        if ($keep.Contains($rel)) { $present++ }
        elseif ($rel -ne '.sync-manifest' -and $rel -notmatch $skip) { $rel }
    })
# Every tracked file was just extracted, so nearly all must be found under their relative paths;
# if they are not, the paths are wrong and nothing may be deleted.
if ($present -lt [math]::Floor($keep.Count * 0.95)) { throw "only $present of $($keep.Count) tracked files found at their paths; refusing to clean" }
if ($stale.Count -gt $Max) { throw "refusing to delete $($stale.Count) files (more than $Max); first: $($stale[0..4] -join ', ')" }
foreach ($rel in $stale) {
    if ($DryRun) { "would remove $rel" } else { Remove-Item -LiteralPath (Join-Path $root $rel) -Force; "removed stale $rel" }
}
"tracked files present: $present of $($keep.Count); stale: $($stale.Count)$(if ($DryRun) { ' (dry run)' })"
