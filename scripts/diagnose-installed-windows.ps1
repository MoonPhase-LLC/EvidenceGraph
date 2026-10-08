# S1-09: collect diagnostics from an INSTALLED EvidenceGraph package.
#
# Use this when validate-installed-windows.ps1 fails, or to record what an
# installed app does. Needs only Windows PowerShell 5.1 (stock Windows): no
# Python, Node, internet access or debugging port. It launches the installed
# app unchanged, watches it from the outside, closes it, and writes a
# timestamped report:
#   powershell -NoProfile -ExecutionPolicy Bypass -File diagnose-installed-windows.ps1
#
# What the shipped build does NOT provide, so this script cannot either:
# - Persistent logs. The release app writes none. app.exe is a GUI program
#   with no console, and it reads the service's stderr and discards it in
#   release builds. The service's own output is therefore not observable.
# - stdout/stderr. app.exe has none to capture: the script reads the PE
#   header to confirm it is a GUI-subsystem program. It does not redirect
#   them: redirection makes the app inherit this script's handles and so
#   changes the launch. The service's stdin/stdout are the private startup
#   channel to app.exe and are deliberately not intercepted.
# - The service's exit code. Windows only reports it to the process that
#   holds a handle opened while it ran; the script opens one when it first
#   sees the service, so it is usually, but not always, captured.
#
# Privacy: no environment variable values, no protocol traffic, no
# credentials. UI text and command lines pass through a filter that hides
# long token-like strings. Only the names of EVIDENCEGRAPH_*/WEBVIEW2_*
# variables are recorded, because they can change behavior.

param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'EvidenceGraph'),
    [int]$ObserveSeconds = 45,
    [string]$ReportPath = (Join-Path $PSScriptRoot ("s1-09-diagnostics-{0}.txt" -f (Get-Date -Format 'yyyyMMdd-HHmmss')))
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:lines = New-Object System.Collections.Generic.List[string]
$t0 = Get-Date
function Log([string]$text) {
    $stamp = '{0,7:N1}s' -f ((Get-Date) - $t0).TotalSeconds
    $line = "$stamp  $text"
    Write-Host $line
    $script:lines.Add($line)
}
function Save { $script:lines | Set-Content -Encoding UTF8 $ReportPath }
# Hides anything that looks like a token or secret (long runs of base64url
# characters), wherever free text from the app is recorded.
function Redact([string]$text) {
    if ($null -eq $text) { return '' }
    return [regex]::Replace($text, '[A-Za-z0-9_\-+/=]{24,}', { param($m) "<redacted:$($m.Value.Length) chars>" })
}

try {
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    $uiaAvailable = $true
} catch {
    $uiaAvailable = $false
}

# --- Environment -----------------------------------------------------------
$os = Get-CimInstance Win32_OperatingSystem
Log "EvidenceGraph S1-09 diagnostics, started $(Get-Date -Format o)"
Log "OS: $($os.Caption) $($os.Version) build $($os.BuildNumber), $([Environment]::Is64BitOperatingSystem -as [string]) 64-bit"
Log "Elevated: $(([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"
$wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
if ($null -eq $wv) { $wv = Get-ItemProperty 'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue }
Log "WebView2 runtime: $(if ($wv) { $wv.pv } else { 'NOT FOUND (the app cannot render without it)' })"
$relevantVars = @(Get-ChildItem Env: | Where-Object { $_.Name -like 'EVIDENCEGRAPH_*' -or $_.Name -like 'WEBVIEW2_*' } | ForEach-Object { $_.Name })
Log "Behavior-relevant environment variable NAMES (values not recorded): $(if ($relevantVars) { $relevantVars -join ', ' } else { 'none' })"
$up = @(Get-NetAdapter -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'Up' } | ForEach-Object { $_.Name })
Log "Network adapters Up: $(if ($up) { $up -join ', ' } else { 'none' })"
Log "UI Automation available: $uiaAvailable"

# --- Installation ----------------------------------------------------------
Log "Install directory: $InstallDir (exists: $(Test-Path $InstallDir))"
$serviceExe = Join-Path $InstallDir 'service\evidencegraph-service.exe'
$appCandidates = @(Get-ChildItem $InstallDir -Filter '*.exe' -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike 'uninstall*' })
foreach ($f in @($appCandidates) + @(Get-Item $serviceExe -ErrorAction SilentlyContinue)) {
    Log ("  {0}  {1} bytes  SHA-256 {2}" -f $f.FullName, $f.Length, (Get-FileHash $f.FullName -Algorithm SHA256).Hash)
}
if (-not (Test-Path $serviceExe)) {
    Log "  SERVICE EXECUTABLE MISSING at $serviceExe (not installed, or removed/quarantined by antivirus)"
}
$internal = Join-Path $InstallDir 'service\_internal'
if (Test-Path $internal) {
    $m = Get-ChildItem $internal -Recurse -File | Measure-Object -Sum Length
    Log "  service\_internal: $($m.Count) files, $($m.Sum) bytes"
} else {
    Log "  service\_internal MISSING"
}
$dataDir = Join-Path $env:LOCALAPPDATA 'com.evidencegraph.app'
Log "App data directory (WebView2 profile only, not logs): $dataDir (exists: $(Test-Path $dataDir))"
Log "Persistent app/service logs: none. The release build writes no log files (see the script header)."

if ($appCandidates.Count -ne 1) {
    Log "Expected exactly one app executable in $InstallDir, found $($appCandidates.Count). Stopping."
    Save; Write-Host "Report: $ReportPath"; exit 1
}
$appExe = $appCandidates[0].FullName
$already = @(Get-Process | Where-Object { $_.Path -eq $appExe })
if ($already.Count -gt 0) {
    Log "An EvidenceGraph app is already running (PID $($already.Id -join ', ')). Close it and run again. Stopping."
    Save; Write-Host "Report: $ReportPath"; exit 1
}

# --- Helpers -----------------------------------------------------------------
function Get-Tree([int]$rootPid) {
    $all = @(Get-CimInstance Win32_Process)
    $ids = @($rootPid); $found = @()
    do {
        $next = @($all | Where-Object { $ids -contains $_.ParentProcessId })
        $ids = @($next | ForEach-Object { $_.ProcessId })
        $found += $next
    } while ($next.Count -gt 0)
    return $found
}
function Describe($p) {
    $kind = $p.Name
    if ($p.Name -eq 'msedgewebview2.exe') {
        $type = if ($p.CommandLine -match '--utility-sub-type=(\S+)') { $Matches[1] } elseif ($p.CommandLine -match '--type=(\S+)') { $Matches[1] } else { 'browser' }
        $kind = "msedgewebview2.exe [$type]"
    }
    return $kind
}
function Get-UiTexts([int]$appPid) {
    if (-not $uiaAvailable) { return @() }
    try {
        $root = [System.Windows.Automation.AutomationElement]::RootElement
        $cond = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $appPid)
        $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
        if ($null -eq $window) { return @() }
        $names = @("[window] $($window.Current.Name)")
        foreach ($e in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                [System.Windows.Automation.Condition]::TrueCondition)) {
            try { $n = $e.Current.Name; if ($n -and $n.Length -le 300) { $names += $n } } catch { }
        }
        return $names
    } catch {
        return @("[UI Automation error: $($_.Exception.GetType().Name)]")
    }
}

# --- Launch and observe ----------------------------------------------------
$cwd = Join-Path $env:TEMP 'eg-s1-09-diagnostics-cwd'
New-Item -ItemType Directory -Force $cwd | Out-Null
$launchTime = Get-Date
Log ''
Log "Launching $appExe from working directory $cwd"
$app = Start-Process -FilePath $appExe -WorkingDirectory $cwd -PassThru
$null = $app.Handle  # keep a handle so the exit code stays readable
Log "app.exe PID $($app.Id)"

$known = @{}          # pid -> description
$handles = @{}        # pid -> System.Diagnostics.Process (for exit codes)
$listenersSeen = @{}  # "addr:port" -> owner description
$lastUi = ''
$deadline = (Get-Date).AddSeconds($ObserveSeconds)
while ((Get-Date) -lt $deadline -and -not $app.HasExited) {
    $tree = @(Get-Tree $app.Id)
    $current = @{}
    foreach ($p in $tree) {
        $id = [int]$p.ProcessId
        $current[$id] = $true
        if (-not $known.ContainsKey($id)) {
            $desc = Describe $p
            $known[$id] = $desc
            if ($p.Name -ne 'msedgewebview2.exe') {
                Log ("START  PID {0} parent {1} {2}  path={3}  cmdline={4}" -f $id, $p.ParentProcessId, $desc, $p.ExecutablePath, (Redact $p.CommandLine))
                try { $h = [System.Diagnostics.Process]::GetProcessById($id); $null = $h.Handle; $handles[$id] = $h } catch { }
            } else {
                Log ("START  PID {0} parent {1} {2}" -f $id, $p.ParentProcessId, $desc)
            }
        }
    }
    foreach ($id in @($known.Keys)) {
        if (-not $current.ContainsKey($id) -and $known[$id] -notlike '*(exited)') {
            $code = if ($handles.ContainsKey($id)) { try { $handles[$id].ExitCode } catch { 'unknown' } } else { 'not captured' }
            Log ("EXIT   PID {0} {1}  exit code: {2}" -f $id, $known[$id], $code)
            $known[$id] = "$($known[$id]) (exited)"
        }
    }
    $ownerIds = @($current.Keys) + @($app.Id)
    foreach ($c in @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $ownerIds -contains [int]$_.OwningProcess })) {
        $key = "$($c.LocalAddress):$($c.LocalPort)"
        if (-not $listenersSeen.ContainsKey($key)) {
            $listenersSeen[$key] = "PID $($c.OwningProcess) $($known[[int]$c.OwningProcess])"
            Log "LISTEN $key  owner $($listenersSeen[$key])"
        }
    }
    $ui = (@(Get-UiTexts $app.Id) | ForEach-Object { Redact $_ }) -join ' | '
    if ($ui -ne $lastUi) {
        Log "UI     $ui"
        $lastUi = $ui
    }
    Start-Sleep -Milliseconds 1000
}

# --- Close and check cleanup ----------------------------------------------
Log ''
$treePids = @(Get-Tree $app.Id | ForEach-Object { [int]$_.ProcessId })
if ($app.HasExited) {
    Log "app.exe exited by itself during observation, exit code $($app.ExitCode)"
} else {
    Log 'Closing the app window (normal close)'
    $null = $app.CloseMainWindow()
    if ($app.WaitForExit(20000)) {
        Log "app.exe exited, exit code $($app.ExitCode)"
    } else {
        Log 'app.exe did NOT exit within 20 s of the close request; forcing termination'
        Stop-Process -Id $app.Id -Force
        $null = $app.WaitForExit(10000)
        Log "app.exe terminated, exit code $($app.ExitCode)"
    }
}
Start-Sleep -Seconds 3
foreach ($id in $handles.Keys) {
    if ($id -eq $app.Id) { continue }
    $h = $handles[$id]
    $state = if ($h.HasExited) { "exited, exit code $(try { $h.ExitCode } catch { 'unknown' })" } else { 'STILL RUNNING' }
    Log ("After close: PID {0} {1}: {2}" -f $id, ($known[$id] -replace ' \(exited\)$', ''), $state)
}
$remaining = @(Get-Process -Id $treePids -ErrorAction SilentlyContinue)
Log "Processes from the app's tree still running after close: $(if ($remaining) { ($remaining | ForEach-Object { "$($_.Id) $($_.ProcessName)" }) -join ', ' } else { 'none' })"
foreach ($key in $listenersSeen.Keys) {
    $port = [int]($key -split ':')[-1]
    $still = @(Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue)
    Log "Listener $key after close: $(if ($still) { "STILL LISTENING (PID $($still[0].OwningProcess))" } else { 'gone' })"
}
if ($listenersSeen.Count -eq 0) { Log 'No listening port was seen for the app or its children.' }

# --- stdout / stderr -------------------------------------------------------
Log ''
function Get-PeSubsystem([string]$path) {
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $pe = [BitConverter]::ToInt32($bytes, 0x3C)
    $sub = [BitConverter]::ToUInt16($bytes, $pe + 24 + 68)  # OptionalHeader.Subsystem
    switch ($sub) { 2 { 'GUI (no console streams)' } 3 { 'console' } default { "other ($sub)" } }
}
Log "app.exe PE subsystem: $(Get-PeSubsystem $appExe)"
if (Test-Path $serviceExe) { Log "evidencegraph-service.exe PE subsystem: $(Get-PeSubsystem $serviceExe)" }
Log 'stdout/stderr: not available. app.exe is a GUI program with no console streams, and in release builds it'
Log 'discards the service stderr it reads. The service''s stdin/stdout carry the private startup protocol'
Log 'and are not intercepted. A console host (conhost.exe) under the service is normal for a console program.'

# --- Windows events ----------------------------------------------------------
Log ''
$names = 'app.exe|evidencegraph-service.exe|msedgewebview2.exe|EvidenceGraph'
try {
    $events = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $launchTime.AddSeconds(-5) } -ErrorAction Stop |
        Where-Object { $_.ProviderName -in @('Application Error', 'Windows Error Reporting', 'Application Hang', '.NET Runtime', 'SideBySide') -and $_.Message -match $names })
    Log "Application-log crash/hang events since launch: $($events.Count)"
    foreach ($e in $events) {
        Log ("  {0:o} {1} id {2}:" -f $e.TimeCreated, $e.ProviderName, $e.Id)
        foreach ($l in (($e.Message -split "`r?`n") | Where-Object { $_.Trim() } | Select-Object -First 12)) { Log "      $l" }
    }
} catch {
    if ($_.Exception.Message -match 'No events were found') { Log 'Application-log crash/hang events since launch: 0' }
    else { Log "Could not read the Application event log: $($_.Exception.Message)" }
}
try {
    $defender = @(Get-WinEvent -FilterHashtable @{ LogName = 'Microsoft-Windows-Windows Defender/Operational'; Id = 1116, 1117, 1118, 1119; StartTime = $t0.AddHours(-24) } -ErrorAction Stop |
        Where-Object { $_.Message -match $names })
    Log "Microsoft Defender detections mentioning EvidenceGraph files (last 24 h): $($defender.Count)"
    foreach ($e in $defender) { Log ("  {0:o} id {1}: {2}" -f $e.TimeCreated, $e.Id, (($e.Message -split "`r?`n") | Select-Object -First 6) -join ' / ') }
} catch {
    if ($_.Exception.Message -match 'No events were found') { Log 'Microsoft Defender detections mentioning EvidenceGraph files (last 24 h): 0' }
    else { Log "Could not read the Defender log (may need administrator rights): $($_.Exception.Message)" }
}
$werDirs = @("$env:LOCALAPPDATA\Microsoft\Windows\WER\ReportArchive", "$env:LOCALAPPDATA\Microsoft\Windows\WER\ReportQueue")
$wer = @(Get-ChildItem $werDirs -Directory -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -ge $launchTime.AddSeconds(-5) -and $_.Name -match 'app\.exe|evidencegraph|msedgewebview2' })
Log "Windows Error Reporting folders created since launch: $(if ($wer) { ($wer | ForEach-Object { $_.FullName }) -join ', ' } else { 'none' })"

Log ''
Log "Finished $(Get-Date -Format o)"
Log 'Not captured by this script (needs a screenshot or your observation): what the window looked like, any Windows dialog or SmartScreen prompt, and anything that happened before this script started (for example during installation).'
Save
Write-Host "Report: $ReportPath"
