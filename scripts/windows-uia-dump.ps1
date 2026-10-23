# Dump the UI Automation tree of the running Common app window: the names, roles and states NVDA and
# Narrator read (a screen-reader-level check without a screen reader; it is not a test with NVDA itself).
# Must run in the signed-in desktop session (a scheduled task with the interactive token, as CommonTestLaunch).
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-uia-dump.ps1 -Out <file> [-Depth 40]
param([Parameter(Mandatory = $true)][string]$Out, [int]$Depth = 40)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$AE = [System.Windows.Automation.AutomationElement]
$p = Get-Process common-desktop -ErrorAction Stop | Select-Object -First 1
$cond = New-Object System.Windows.Automation.PropertyCondition($AE::ProcessIdProperty, $p.Id)
$win = $AE::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
if (-not $win) { throw "no top-level window for common-desktop $($p.Id)" }
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
$lines = New-Object System.Collections.Generic.List[string]
function Walk($el, $d) {
    if ($d -gt $Depth) { return }
    try {
        $c = $el.Current
        $live = try { $el.GetCurrentPropertyValue($AE::LiveSettingProperty) } catch { "" }
        $extra = @()
        if ($c.AutomationId) { $extra += "id=$($c.AutomationId)" }
        if ($c.HelpText) { $extra += "help=$($c.HelpText)" }
        if ($c.ItemStatus) { $extra += "status=$($c.ItemStatus)" }
        if ($c.IsKeyboardFocusable) { $extra += "focusable" }
        if ($c.HasKeyboardFocus) { $extra += "FOCUSED" }
        if (-not $c.IsEnabled) { $extra += "disabled" }
        if ($c.IsOffscreen) { $extra += "offscreen" }
        if ($live -and "$live" -ne "Off") { $extra += "live=$live" }
        $lines.Add(("  " * $d) + $c.LocalizedControlType + " `"" + $c.Name + "`" " + ($extra -join " "))
        $ch = $walker.GetFirstChild($el)
        while ($ch) { Walk $ch ($d + 1); $ch = $walker.GetNextSibling($ch) }
    } catch {
        # ponytail: an element removed mid-walk (a React re-render) loses only its own subtree or its later
        # siblings; the line says so, and the rest of the dump is still written.
        $lines.Add(("  " * $d) + "<element gone: $($_.Exception.Message)>")
    }
}
Walk $win 0
$lines | Set-Content -Encoding utf8 $Out
"UIA_DONE $($lines.Count) elements" | Add-Content -Encoding utf8 $Out
