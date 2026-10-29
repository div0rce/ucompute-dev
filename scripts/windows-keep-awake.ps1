# Run a command while asking Windows not to sleep (the equivalent of macOS `caffeinate -i`): the
# process holds ES_SYSTEM_REQUIRED through SetThreadExecutionState until the command exits. Changes no
# power settings; the request ends with this process.
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-keep-awake.ps1 -Command "<cmd.exe command line>"
# -Forever holds the request until this process is stopped (the UComputeKeepAwake logon task runs it that way;
# register it with -Install, remove it with -Uninstall).
param([string]$Command, [switch]$Forever, [switch]$Install, [switch]$Uninstall)
$ErrorActionPreference = "Stop"
if ($Install -or $Uninstall) {
    Unregister-ScheduledTask -TaskName UComputeKeepAwake -Confirm:$false -ErrorAction SilentlyContinue
    if ($Uninstall) { return }
    $a = New-ScheduledTaskAction -Execute powershell.exe -Argument "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Forever"
    $s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName UComputeKeepAwake -Action $a -Trigger (New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME) -Settings $s | Out-Null
    Start-ScheduledTask -TaskName UComputeKeepAwake
    return
}
if (-not $Forever -and -not $Command) { throw "give -Command, -Forever, -Install or -Uninstall" }
Add-Type -Namespace UCompute -Name Power -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll")]
public static extern uint SetThreadExecutionState(uint esFlags);
'@
$ES_CONTINUOUS = [uint32]"0x80000000"
$ES_SYSTEM_REQUIRED = [uint32]"0x00000001"
# On a Modern Standby (S0 low power idle) laptop the display turning off enters standby whatever the system
# request says, so the -Forever hold also keeps the display on (ES_DISPLAY_REQUIRED). The screen stays lit;
# Win+L still locks it.
$ES_DISPLAY_REQUIRED = [uint32]"0x00000002"
$flags = $ES_CONTINUOUS -bor $ES_SYSTEM_REQUIRED
if ($Forever) { $flags = $flags -bor $ES_DISPLAY_REQUIRED }
[void][UCompute.Power]::SetThreadExecutionState($flags)
try {
    if ($Forever) { [System.Threading.Thread]::Sleep([System.Threading.Timeout]::Infinite) }
    # /s: cmd strips only the outer quote pair we add, so a command with several quoted paths runs as written.
    $p = Start-Process -FilePath cmd.exe -ArgumentList "/d /s /c `"$Command`"" -NoNewWindow -PassThru
    $null = $p.Handle  # keep the handle, or ExitCode reads empty once the process has exited
    # Wait for the command only: Start-Process -Wait would also wait for anything it left running in the background.
    $p.WaitForExit()
    exit $p.ExitCode
} finally {
    [void][UCompute.Power]::SetThreadExecutionState($ES_CONTINUOUS)
}
