# Verify the Windows build of the Common desktop app on the exact installer, end to end.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File apps\desktop\scripts\verify_windows_app.ps1 `
#       -Installer <...\nsis\Common_0.1.0_x64-setup.exe> -Work <empty folder>
#
# 1. Silent per-user install into <Work>\app (NSIS /S /D=...), never over an existing install; THIRD_PARTY_NOTICES.txt
#    next to the exe names rustls and react.
# 2. First launch with a fresh data folder (LOCALAPPDATA=<Work>\localappdata): the app's daemon becomes healthy on
#    127.0.0.1:8192, is the bundled common.exe started by the app with src-tauri\daemon-args.txt, and downloads
#    the pinned engine into the fresh folder.
# 3. Real inference through the app's daemon: pull the stories15M fixture, load it, check the golden completion.
# 4. Quit (close the main window when it has one), restart, crash (Stop-Process -Force on the app): the daemon and
#    its engine end with the app every time and port 8192 is released.
# Writes one line per check and <Work>\verify.json. Needs network access and a free port 8192.
param(
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][string]$Work
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$Golden = " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big"
$Model = "stories15M-q4_0"
$Base = "http://127.0.0.1:8192"
$Steps = New-Object System.Collections.ArrayList
$DaemonArgs = (Get-Content (Join-Path $PSScriptRoot "..\src-tauri\daemon-args.txt") -Raw).Trim() -split "\s+"

function Step($name, $ok, $facts) {
    [void]$Steps.Add([ordered]@{ step = $name; ok = [bool]$ok; facts = $facts })
    $tag = if ($ok) { "PASS" } else { "FAIL" }
    Write-Output ("{0} {1} {2}" -f $tag, $name, ($facts | ConvertTo-Json -Compress -Depth 4))
    if (-not $ok) { Finish; exit 1 }
}
function Finish { $Steps | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $Work "verify.json") }
function Healthy { try { (Invoke-WebRequest "$Base/health" -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200 } catch { $false } }
function PortOpen { [bool](Get-NetTCPConnection -LocalPort 8192 -State Listen -ErrorAction SilentlyContinue) }
function WaitFor([scriptblock]$cond, [int]$seconds) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $seconds) { if (& $cond) { return [math]::Round($sw.Elapsed.TotalSeconds, 2) }; Start-Sleep -Milliseconds 200 }
    return $null
}
function Procs($name) { @(Get-CimInstance Win32_Process -Filter "Name='$name'") }
function Alive($id) { [bool](Get-Process -Id $id -ErrorAction SilentlyContinue) }

if ((Test-Path $Work) -and (Get-ChildItem $Work -Force | Select-Object -First 1)) { throw "-Work must be empty: $Work" }
New-Item -ItemType Directory -Force $Work | Out-Null
if (PortOpen) { throw "port 8192 is in use; stop any running Common daemon first (the app would adopt it)" }

# 1. install
$appDir = Join-Path $Work "app"
$data = Join-Path $Work "localappdata"
New-Item -ItemType Directory -Force $data | Out-Null
$hash = (Get-FileHash $Installer -Algorithm SHA256).Hash.ToLower()
$p = Start-Process -FilePath $Installer -ArgumentList "/S", "/D=$appDir" -PassThru -Wait
$exe = Get-Item (Join-Path $appDir "common-desktop.exe") -ErrorAction SilentlyContinue  # the cargo binary name, as on macOS
$daemonExe = Join-Path $appDir "common.exe"
Step "silent install" ($p.ExitCode -eq 0 -and $exe -and (Test-Path $daemonExe)) @{ installer_sha256 = $hash; app = "$($exe.Name)"; files = @(Get-ChildItem $appDir -Name) }
# Written at build time by scripts/gen_notices.py (build-sidecar.mjs): one entry line per package, `<name> <version> ...`.
$notices = Join-Path $appDir "THIRD_PARTY_NOTICES.txt"
$nt = if (Test-Path $notices) { Get-Content $notices -Raw -Encoding UTF8 } else { "" }
Step "the install ships THIRD_PARTY_NOTICES next to the exe, naming rustls and react" (($nt -match '(?m)^rustls \d') -and ($nt -match '(?m)^react \d')) @{ path = $notices; chars = $nt.Length }
$help = Start-Process -FilePath $daemonExe -ArgumentList ($DaemonArgs + "--help") -NoNewWindow -PassThru -Wait -RedirectStandardOutput (Join-Path $Work "help.txt")
Step "bundled daemon accepts the app's exact arguments" ($help.ExitCode -eq 0) @{ args = $DaemonArgs; exit = $help.ExitCode }

function Launch {
    $psi = New-Object System.Diagnostics.ProcessStartInfo $exe.FullName
    $psi.UseShellExecute = $false
    $psi.EnvironmentVariables["LOCALAPPDATA"] = $data
    $psi.WorkingDirectory = $appDir
    [System.Diagnostics.Process]::Start($psi)
}

# 2. first launch
$app = Launch
$t = WaitFor { Healthy } 180
# @(): on Windows PowerShell 5.1 a single CimInstance has no .Count, so one correct daemon read as "not one"
$daemons = @(Procs "common.exe" | Where-Object { $_.ExecutablePath -eq $daemonExe })
$argsOk = $daemons.Count -eq 1 -and $daemons[0].ParentProcessId -eq $app.Id -and ($daemons[0].CommandLine -replace '^"[^"]+"\s*|^\S+\s*', '') -eq ($DaemonArgs -join " ")
Step "first launch: daemon healthy, started by the app with its exact arguments" ($t -ne $null -and $argsOk) @{ healthy_after_s = $t; daemon = @($daemons | ForEach-Object { "$($_.ProcessId) parent $($_.ParentProcessId): $($_.CommandLine)" }) }
$engineDir = Join-Path $data "Common\backends\llama.cpp"
Step "fresh install provisioned the pinned engine in the fresh data folder" (Test-Path (Join-Path $engineDir "b11609\llama-server.exe")) @{ dir = $engineDir }

# 3. real inference
$sw = [Diagnostics.Stopwatch]::StartNew()
$pull = Invoke-WebRequest "$Base/api/models/$Model/pull" -Method Post -UseBasicParsing -TimeoutSec 600
$done = ($pull.Content -split "`n" | Where-Object { $_ -like "data:*" } | Select-Object -Last 1) -replace "^data:\s*", "" | ConvertFrom-Json
Step "model pulled and hash-verified by the app's daemon" ($done.done -eq $true) @{ bytes = $done.downloaded_bytes; seconds = [math]::Round($sw.Elapsed.TotalSeconds, 1) }
function Infer($tag) {
    $load = Invoke-RestMethod "$Base/api/models/$Model/load" -Method Post -ContentType "application/json" -Body '{"device":"auto"}' -TimeoutSec 300
    $body = @{ model = $Model; prompt = "Once upon a time, a little rabbit"; max_tokens = 32; temperature = 0 } | ConvertTo-Json
    $out = Invoke-RestMethod "$Base/v1/completions" -Method Post -ContentType "application/json" -Body $body -TimeoutSec 300
    Step "${tag}: real inference through the app's daemon" ($out.choices[0].text -eq $Golden) @{ device = $load.device_actual; decode_tok_s = [math]::Round($out.timings.decode_tokens_per_second, 1); text = $out.choices[0].text }
}
Infer "first launch"

function Ends($why) {
    $d = @(Procs "common.exe" | Where-Object { $_.ExecutablePath -eq $daemonExe } | ForEach-Object { $_.ProcessId })
    $e = @(Procs "llama-server.exe" | Where-Object { $_.ExecutablePath -like "$data*" } | ForEach-Object { $_.ProcessId })
    return @{ daemons = $d; engines = $e }
}

# 4a. quit
$before = Ends
$closed = $app.CloseMainWindow()
if (-not $closed) { Stop-Process -Id $app.Id }
$tApp = WaitFor { $app.HasExited } 20
$tGone = WaitFor { -not ($before.daemons + $before.engines | Where-Object { Alive $_ }) } 15
$tPort = WaitFor { -not (PortOpen) } 10
Step "quit: daemon and engine end with the app, port released" ($tApp -ne $null -and $tGone -ne $null -and $tPort -ne $null) @{ how = $(if ($closed) { "CloseMainWindow" } else { "Stop-Process (no main window in this session)" }); app_exit_s = $tApp; children_gone_s = $tGone; before = $before }

# 4b. restart
$app = Launch
$t = WaitFor { Healthy } 60
$now = Ends
Step "restart: one daemon from the new app, no port conflict" ($t -ne $null -and $now.daemons.Count -eq 1) @{ healthy_after_s = $t; daemons = $now.daemons }
Infer "restart"

# 4c. crash
$before = Ends
Stop-Process -Id $app.Id -Force
$tGone = WaitFor { -not ($before.daemons + $before.engines | Where-Object { Alive $_ }) } 10
$tPort = WaitFor { -not (PortOpen) } 10
Step "crash (Stop-Process -Force on the app): daemon and engine end with it" ($tGone -ne $null -and $tPort -ne $null) @{ children_gone_s = $tGone; before = $before }

# 4d. restart after the crash, then quit; nothing left
$app = Launch
$t = WaitFor { Healthy } 60
Step "restart after the crash" ($t -ne $null) @{ healthy_after_s = $t }
$before = Ends
Stop-Process -Id $app.Id -Force
$tGone = WaitFor { -not ($before.daemons + $before.engines | Where-Object { Alive $_ }) } 10
$left = @(Procs "common.exe" | Where-Object { $_.ExecutablePath -eq $daemonExe }) + @(Procs "llama-server.exe" | Where-Object { $_.ExecutablePath -like "$data*" })
Step "final: nothing from the install is running, port free" ($tGone -ne $null -and $left.Count -eq 0 -and -not (PortOpen)) @{ leftovers = @($left | ForEach-Object { $_.ProcessId }) }
Finish
Write-Output "ALL PASS"
