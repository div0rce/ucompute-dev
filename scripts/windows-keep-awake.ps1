# Run a command while asking Windows not to sleep (the equivalent of macOS `caffeinate -i`): the
# process holds ES_SYSTEM_REQUIRED through SetThreadExecutionState until the command exits. Changes no
# power settings; the request ends with this process.
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-keep-awake.ps1 -Command "<cmd.exe command line>"
param([Parameter(Mandatory = $true)][string]$Command)
$ErrorActionPreference = "Stop"
Add-Type -Namespace Common -Name Power -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll")]
public static extern uint SetThreadExecutionState(uint esFlags);
'@
$ES_CONTINUOUS = [uint32]"0x80000000"
$ES_SYSTEM_REQUIRED = [uint32]"0x00000001"
[void][Common.Power]::SetThreadExecutionState($ES_CONTINUOUS -bor $ES_SYSTEM_REQUIRED)
try {
    # /s: cmd strips only the outer quote pair we add, so a command with several quoted paths runs as written.
    $p = Start-Process -FilePath cmd.exe -ArgumentList "/d /s /c `"$Command`"" -NoNewWindow -PassThru
    $null = $p.Handle  # keep the handle, or ExitCode reads empty once the process has exited
    # Wait for the command only: Start-Process -Wait would also wait for anything it left running in the background.
    $p.WaitForExit()
    exit $p.ExitCode
} finally {
    [void][Common.Power]::SetThreadExecutionState($ES_CONTINUOUS)
}
