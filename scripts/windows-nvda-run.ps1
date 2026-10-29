# NVDA (2026.2 portable; sha256 f3f8d299…ca824, nvaccess.org release post) speech capture for the UCompute app on a test laptop (a test with NVDA itself, silent):
#   start: refuse unless the signed-in user has been idle -MinIdle seconds and no NVDA runs; start a portable
#          NVDA with a seeded config (silence synth, no sounds, no browse mode on page load, no update or
#          add-on network checks) logging at level IO (12) to a raw log; bring the app window to the front.
#   stop:  quit NVDA, confirm it exited, copy ONLY the "Speaking" entries to -Speech, delete the raw logs.
# Level IO also logs every key typed anywhere in the session (unmasked): the raw log never leaves this
# machine and is deleted at stop; the idle check keeps runs to an unattended laptop. Must run in the
# signed-in desktop session (a scheduled task with the interactive token); the app must be running.
#   powershell -NoProfile -ExecutionPolicy Bypass -File windows-nvda-run.ps1 start|stop -Nvda <portable dir> -Work <dir>
param(
    [Parameter(Mandatory = $true)][ValidateSet("start", "stop")][string]$Action,
    [Parameter(Mandatory = $true)][string]$Nvda,
    [Parameter(Mandatory = $true)][string]$Work,
    [int]$MinIdle = 300,
    [string]$Window = "UCompute"
)
$ErrorActionPreference = "Stop"
$exe = Join-Path $Nvda "nvda.exe"
$cfg = Join-Path $Work "nvda-config"
$raw = Join-Path $Work "nvda-raw.log"
$speech = Join-Path $Work "nvda-speech.txt"
$status = Join-Path $Work "nvda-status.txt"
function Running { (Start-Process $exe -ArgumentList "-k" -Wait -PassThru).ExitCode -eq 0 }

if ($Action -eq "start") {
    Add-Type -Namespace UCompute -Name Idle -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)] public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
[DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO plii);
'@
    $i = New-Object UCompute.Idle+LASTINPUTINFO
    $i.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf($i)
    [void][UCompute.Idle]::GetLastInputInfo([ref]$i)
    $idle = [math]::Round(([Environment]::TickCount - $i.dwTime) / 1000)
    if ($idle -lt $MinIdle) { "REFUSED idle_s=$idle (someone is using this computer)" | Set-Content $status; exit 3 }
    if (Running) { "REFUSED an NVDA is already running in this session" | Set-Content $status; exit 3 }
    New-Item -ItemType Directory -Force $cfg | Out-Null
    # schemaVersion 24 = NVDA 2026.2; an ini that fails validation falls back to defaults (audible), so
    # start checks the log for the silence synth before anything else happens.
    @"
schemaVersion = 24
[general]
	saveConfigurationOnExit = False
	playStartAndExitSounds = False
	loggingLevel = IO
	showWelcomeDialogAtStartup = False
[speech]
	synth = silence
[audio]
	soundVolume = 0
	soundVolumeFollowsVoice = False
	audioAwakeTime = 0
[presentation]
	reportAutoSuggestionsWithSound = False
	[[progressBarUpdates]]
		progressBarOutputMode = off
[mouse]
	enableMouseTracking = False
[keyboard]
	beepForLowercaseWithCapslock = False
	speakTypedCharacters = 0
	speakTypedWords = 0
[virtualBuffers]
	enableOnPageLoad = False
	autoSayAllOnPageLoad = False
	passThroughAudioIndication = False
[update]
	autoCheck = False
	startupNotification = False
	allowUsageStats = False
	askedAllowUsageStats = True
	serverURL = http://127.0.0.1:9
[addonStore]
	automaticUpdates = disabled
	baseServerURL = http://127.0.0.1:9
[remote]
	enabled = False
[featureFlag]
	playErrorSound = 2
[screenCurtain]
	playToggleSounds = False
"@ | Set-Content -Encoding utf8 (Join-Path $cfg "nvda.ini")
    Remove-Item $raw, $speech -ErrorAction SilentlyContinue
    Start-Process $exe -ArgumentList "--minimal", "--disable-addons", "--lang=en", "--config-path=`"$cfg`"", "--log-file=`"$raw`"", "--log-level=12"
    $ok = $false
    for ($n = 0; $n -lt 40 -and -not $ok; $n++) {
        Start-Sleep -Milliseconds 500
        if ((Test-Path $raw) -and (Select-String -Path $raw -Pattern "Error loading base configuration|Loaded synthDriver (?!silence)" -Quiet)) { break }
        $ok = (Test-Path $raw) -and (Select-String -Path $raw -SimpleMatch "NVDA initialized" -Quiet) -and
            (Select-String -Path $raw -SimpleMatch "Loaded synthDriver silence" -Quiet)
    }
    if (-not $ok) {
        Start-Process $exe -ArgumentList "-q" -Wait
        Remove-Item $raw -ErrorAction SilentlyContinue
        "FAILED NVDA did not load the silence synthesizer" | Set-Content $status; exit 4
    }
    $shell = New-Object -ComObject WScript.Shell
    $front = $shell.AppActivate($Window)
    "READY idle_s=$idle foreground=$front $(Get-Date -Format o)" | Set-Content $status
    exit 0
}

Start-Process $exe -ArgumentList "-q" -Wait
$gone = -not (Running)
# Keep only the speech: an entry is "IO - speech.speech.speak (HH:MM:SS.mmm) - ..." then "Speaking [...]".
$lines = if (Test-Path $raw) { Get-Content -Encoding utf8 $raw } else { @() }
$out = New-Object System.Collections.Generic.List[string]
for ($n = 1; $n -lt $lines.Count; $n++) {
    if ($lines[$n].StartsWith("Speaking ") -and $lines[$n - 1] -match '^IO - speech\.speech\.speak \(([0-9:.]+)\)') {
        $out.Add("$($Matches[1]) $($lines[$n])")
    }
}
$out | Set-Content -Encoding utf8 $speech
Remove-Item $raw, (Join-Path $Work "nvda-raw-old.log") -ErrorAction SilentlyContinue
"STOPPED exited=$gone speech_lines=$($out.Count) raw_deleted=$(-not (Test-Path $raw)) $(Get-Date -Format o)" | Set-Content $status
