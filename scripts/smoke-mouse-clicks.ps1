# Smoke test for real mouse clicks on Keeplight's themed buttons. UI
# Automation's Invoke skips the mouse path, so a button can pass other tests
# while ignoring clicks (the theme bug fixed in #4). Posts mouse messages
# straight to the window (the real cursor never moves) against a fake OpenRGB
# server and a throwaway setup folder, and exits 1 if a click does not act.
# The "Keep" buttons start without keyboard focus, which is the case the bug
# broke. Adapted from Modlist Launcher's scripts/smoke-mouse-clicks.ps1.
#
#   ./scripts/smoke-mouse-clicks.ps1 [-Exe target\release\keeplight.exe] [-Fake target\release\examples\fake-openrgb.exe]
param(
    [string] $Exe = (Join-Path $PSScriptRoot '..\target\release\keeplight.exe'),
    [string] $Fake = (Join-Path $PSScriptRoot '..\target\release\examples\fake-openrgb.exe')
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class SmokeClicks {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
}
"@
[SmokeClicks]::SetProcessDpiAwarenessContext([IntPtr]-4) | Out-Null
$A = [System.Windows.Automation.AutomationElement]

$work = Join-Path ([IO.Path]::GetTempPath()) "keeplight-clicks-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$setup = Join-Path $work 'setup.json'

# The fake server prints its address on its first line.
$fpsi = [System.Diagnostics.ProcessStartInfo]::new((Resolve-Path $Fake))
$fpsi.RedirectStandardOutput = $true
$fpsi.UseShellExecute = $false
$fpsi.CreateNoWindow = $true
$server = [System.Diagnostics.Process]::Start($fpsi)
$p = $null
$failed = $false
try {
    $address = $server.StandardOutput.ReadLine()
    if (-not $address) { throw 'the fake OpenRGB server did not start' }

    $psi = [System.Diagnostics.ProcessStartInfo]::new((Resolve-Path $Exe))
    $psi.Environment['KEEPLIGHT_OPENRGB'] = $address
    $psi.Environment['KEEPLIGHT_DATA'] = $work
    $p = [System.Diagnostics.Process]::Start($psi)

    $h = [IntPtr]::Zero
    for ($i = 0; $i -lt 50 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 200; $p.Refresh(); $h = $p.MainWindowHandle }
    if ($h -eq [IntPtr]::Zero) { throw 'the Keeplight window did not appear' }
    [SmokeClicks]::MoveWindow($h, 60, 60, 900, 720, $true) | Out-Null
    Start-Sleep -Milliseconds 1500
    $byPid = New-Object System.Windows.Automation.PropertyCondition ($A::ProcessIdProperty), $p.Id

    # The automation tree can come back empty on the first query.
    function All {
        for ($k = 0; $k -lt 10; $k++) {
            $r = $A::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Children, $byPid)
            $all = @($r.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition))
            if ($all.Count) { return $all }
            Start-Sleep -Milliseconds 500
        }
        @()
    }
    function Has($pattern) { [bool](All | Where-Object { $_.Current.Name -like $pattern }) }
    # Polls for up to 6 seconds: the window refreshes every 3.
    function Eventually([scriptblock] $test) {
        for ($k = 0; $k -lt 12; $k++) { if (& $test) { return $true }; Start-Sleep -Milliseconds 500 }
        $false
    }
    function Click($name) {
        # Wait until the control is laid out where it will stay.
        $b = $null
        $e = $null
        for ($k = 0; $k -lt 10; $k++) {
            $e = All | Where-Object { $_.Current.Name -eq $name } | Select-Object -First 1
            if ($e) {
                $r = $e.Current.BoundingRectangle
                if ($b -and $r -eq $b -and $r.Width -gt 0) { break }
                $b = $r
            }
            Start-Sleep -Milliseconds 300
        }
        if (-not $e) { throw "no control named '$name'" }
        $pt = New-Object SmokeClicks+POINT
        $pt.X = [int]($b.X + $b.Width / 2); $pt.Y = [int]($b.Y + $b.Height / 2)
        [SmokeClicks]::ScreenToClient($h, [ref]$pt) | Out-Null
        $l = [IntPtr](($pt.Y -shl 16) -bor ($pt.X -band 0xffff))
        [SmokeClicks]::PostMessage($h, 0x0200, [IntPtr]0, $l) | Out-Null   # WM_MOUSEMOVE
        Start-Sleep -Milliseconds 120
        [SmokeClicks]::PostMessage($h, 0x0201, [IntPtr]1, $l) | Out-Null   # WM_LBUTTONDOWN
        Start-Sleep -Milliseconds 60
        [SmokeClicks]::PostMessage($h, 0x0202, [IntPtr]0, $l) | Out-Null   # WM_LBUTTONUP
        Start-Sleep -Milliseconds 600
    }
    function Check($label, [bool] $ok) {
        if (-not $ok) { $script:failed = $true }
        '{0,-46} {1}' -f $label, $(if ($ok) { 'ok' } else { 'FAIL' })
    }
    function Kept {
        if (-not (Test-Path $setup)) { return @() }
        @((Get-Content $setup -Raw | ConvertFrom-Json).devices)
    }

    Check 'window lists the fake devices' (Eventually { Has 'Keep RAM stick 2' })
    Click 'Keep RAM stick 2'
    Check 'button: Keep saves that device' (Eventually { (Kept).Count -eq 1 -and (Kept)[0].location -eq 'SMBus 0x73' })
    Check 'button: Keep turns into a Kept status' (Eventually { -not (Has 'Keep RAM stick 2') })
    Click 'Keep Keyboard'
    Check 'button: a second Keep adds the keyboard' (Eventually { (Kept).Count -eq 2 })
} finally {
    if ($p -and -not $p.HasExited) { $p.Kill(); $p.WaitForExit() }
    if (-not $server.HasExited) { $server.Kill(); $server.WaitForExit() }
}
if ($failed) { exit 1 }
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
exit 0
