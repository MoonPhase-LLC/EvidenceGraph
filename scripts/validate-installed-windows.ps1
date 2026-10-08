# S1-09: validate an INSTALLED EvidenceGraph package (see docs/PACKAGING.md).
#
# Meant for the clean-machine check: needs only Windows PowerShell 5.1, which
# ships with Windows. No Python, Node, Rust or repository is required. Run it
# after installing with the NSIS installer, with the machine's network
# disconnected:
#   powershell -NoProfile -ExecutionPolicy Bypass -File validate-installed-windows.ps1
#
# It reads the app's own UI through Windows UI Automation (the accessibility
# tree WebView2 exposes). The shipped build has no debugging port, and this
# script does not enable one.
#
# Exit code 0 only when every check passes. A report is written next to the
# script (or to -ReportPath).

param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'EvidenceGraph'),
    [string]$ReportPath = (Join-Path $PSScriptRoot ("s1-09-validation-{0}.txt" -f (Get-Date -Format 'yyyyMMdd-HHmmss')))
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes

$script:failures = 0
$script:lines = New-Object System.Collections.Generic.List[string]
function Log([string]$text) { Write-Host $text; $script:lines.Add($text) }
function Check([string]$name, [bool]$ok, [string]$detail = '') {
    $mark = if ($ok) { 'PASS' } else { $script:failures++; 'FAIL' }
    Log ("[{0}] {1}{2}" -f $mark, $name, $(if ($detail) { " -- $detail" } else { '' }))
}
function WaitUntil([scriptblock]$condition, [int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        if (& $condition) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return [bool](& $condition)
}

# --- Process helpers -------------------------------------------------------
function Get-Descendants([int]$rootPid) {
    $all = @(Get-CimInstance Win32_Process)
    $ids = @($rootPid); $found = @()
    do {
        $next = @($all | Where-Object { $ids -contains $_.ParentProcessId })
        $ids = @($next | ForEach-Object { $_.ProcessId })
        $found += $next
    } while ($next.Count -gt 0)
    return $found
}
function Get-ServiceProcesses([int]$appPid) {
    @(Get-Descendants $appPid | Where-Object { $_.Name -eq 'evidencegraph-service.exe' })
}
function Get-Alive([int[]]$pids) {
    if ($pids.Count -eq 0) { return @() }
    @(Get-Process -Id $pids -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
}
function Get-Listeners([int[]]$pids) {
    if ($pids.Count -eq 0) { return @() }
    @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess })
}

# --- UI Automation helpers -------------------------------------------------
function Get-UiTexts([int]$appPid) {
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $cond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $appPid)
    $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $cond)
    if ($null -eq $window) { return @() }
    $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition)
    $names = @()
    foreach ($e in $all) {
        try { $n = $e.Current.Name; if ($n) { $names += $n } } catch { }
    }
    return $names
}
function Test-UiText([int]$appPid, [string]$pattern) {
    [bool](@(Get-UiTexts $appPid) | Where-Object { $_ -like $pattern })
}

# --- Launch / close --------------------------------------------------------
$appExe = $null
$serviceExe = Join-Path $InstallDir 'service\evidencegraph-service.exe'
function Start-App {
    # Launch from a directory unrelated to the installation (requirement:
    # resolution must not depend on the current working directory).
    $cwd = Join-Path $env:TEMP 'eg-s1-09-unrelated-cwd'
    New-Item -ItemType Directory -Force $cwd | Out-Null
    return Start-Process -FilePath $appExe -WorkingDirectory $cwd -PassThru
}
function Wait-Ready([System.Diagnostics.Process]$proc) {
    $ready = WaitUntil { Test-UiText $proc.Id 'Service ready' } 60
    $health = $ready -and (WaitUntil { Test-UiText $proc.Id 'Health check passed: evidencegraph-service*' } 30)
    return @{ Ready = $ready; Health = $health }
}

# ===========================================================================
Log "EvidenceGraph S1-09 installed-package validation, $(Get-Date -Format o)"
Log "Computer: $env:COMPUTERNAME  OS: $((Get-CimInstance Win32_OperatingSystem).Caption) $((Get-CimInstance Win32_OperatingSystem).Version)"
Log "User: $env:USERNAME  Elevated: $(([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"
$wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
if ($null -eq $wv) { $wv = Get-ItemProperty 'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue }
Log "WebView2 runtime: $(if ($wv) { $wv.pv } else { 'not found' })"

# Environment: no development toolchain, network disconnected.
foreach ($tool in 'python', 'python3', 'py', 'node', 'npm', 'cargo', 'rustc', 'rustup', 'uv') {
    $cmd = Get-Command $tool -ErrorAction SilentlyContinue
    # The Microsoft Store "python" alias stub is not a Python installation.
    $isStoreStub = $cmd -and $cmd.Source -like '*\WindowsApps\*'
    Check "no '$tool' on PATH" ((-not $cmd) -or $isStoreStub) $(if ($cmd) { $cmd.Source } else { '' })
}
$upAdapters = @(Get-NetAdapter -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'Up' })
Check 'network disconnected (no adapter Up)' ($upAdapters.Count -eq 0) (($upAdapters | ForEach-Object { $_.Name }) -join ', ')

# Installation contents.
$appCandidates = @(Get-ChildItem $InstallDir -Filter '*.exe' -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -notlike 'uninstall*' })
Check "app executable installed in $InstallDir" ($appCandidates.Count -eq 1) (($appCandidates | ForEach-Object { $_.Name }) -join ', ')
Check 'bundled service installed at <install>\service\evidencegraph-service.exe' (Test-Path $serviceExe -PathType Leaf)
if ($appCandidates.Count -ne 1 -or -not (Test-Path $serviceExe)) {
    Log 'Cannot continue without the installed files.'
    $script:lines | Set-Content -Encoding UTF8 $ReportPath
    exit 1
}
$appExe = $appCandidates[0].FullName
Log "App SHA-256:     $((Get-FileHash $appExe -Algorithm SHA256).Hash)  $appExe"
Log "Service SHA-256: $((Get-FileHash $serviceExe -Algorithm SHA256).Hash)  $serviceExe"
Check 'no app process already running' (@(Get-Process | Where-Object { $_.Path -eq $appExe }).Count -eq 0)

function Stop-LeftoverApps {
    foreach ($left in @(Get-Process | Where-Object { $_.Path -eq $appExe })) {
        $left.CloseMainWindow() | Out-Null
        if (-not $left.WaitForExit(15000)) { Stop-Process -Id $left.Id -Force }
    }
}

try {
# --- 1. Launch, identity verification, authenticated health -------------
Log ''
Log '1. Launch from an unrelated directory; service ready; authenticated health shown'
$p = Start-App
$r = Wait-Ready $p
Check "UI shows 'Service ready' (handshake + identity verification complete)" $r.Ready
Check "UI shows authenticated health result from evidencegraph-service" $r.Health
$svc = @(Get-ServiceProcesses $p.Id)
Check 'exactly one bundled service process (no extra bootloader process)' ($svc.Count -eq 1) "$($svc.Count) found"
$svcPids = @($svc | ForEach-Object { [int]$_.ProcessId })
foreach ($s in $svc) {
    Check "service runs from the installation, not PATH/cwd" ($s.ExecutablePath -eq $serviceExe) $s.ExecutablePath
    Check "service command line carries no secret (only --supervised)" ($s.CommandLine -match '--supervised\s*$') $s.CommandLine
}
$listen = @(Get-Listeners $svcPids)
Check 'service listens on loopback only' ($listen.Count -ge 1 -and @($listen | Where-Object { $_.LocalAddress -ne '127.0.0.1' }).Count -eq 0) (($listen | ForEach-Object { "$($_.LocalAddress):$($_.LocalPort)" }) -join ', ')
# Outbound connections: app.exe and the service must make none. The WebView2
# runtime's own processes may contact Microsoft services when a network is
# available (observed during S1-09); they are reported, not failed, and are
# expected to be absent here because the network should be disconnected.
$appTree = @{ $p.Id = 'app.exe' }
foreach ($d in @(Get-Descendants $p.Id)) { $appTree[[int]$d.ProcessId] = $d.Name }
$external = @(Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object {
    $appTree.ContainsKey([int]$_.OwningProcess) -and $_.State -ne 'Listen' -and
    $_.RemoteAddress -notin @('127.0.0.1', '::1', '0.0.0.0', '::') })
$ours = @($external | Where-Object { $appTree[[int]$_.OwningProcess] -ne 'msedgewebview2.exe' })
$webview = @($external | Where-Object { $appTree[[int]$_.OwningProcess] -eq 'msedgewebview2.exe' })
Check 'no non-loopback connections from app.exe or the service' ($ours.Count -eq 0) (($ours | ForEach-Object { "$($appTree[[int]$_.OwningProcess]) -> $($_.RemoteAddress):$($_.RemotePort)" }) -join ', ')
Log ("[NOTE] WebView2 runtime non-loopback connections: {0}" -f $(if ($webview.Count) { ($webview | ForEach-Object { "$($_.RemoteAddress):$($_.RemotePort)" }) -join ', ' } else { 'none' }))

# --- 2. Service crash fails closed and clears health ----------------------
Log ''
Log '2. Service crash fails closed and clears stale health'
Stop-Process -Id $svcPids -Force
$unavailable = WaitUntil { Test-UiText $p.Id 'Service unavailable' } 15
Check "UI shows 'Service unavailable' after the service is killed" $unavailable
Check 'stale health result is cleared' (-not (Test-UiText $p.Id 'Health check passed*'))
Check 'no service process left after crash' (@(Get-ServiceProcesses $p.Id).Count -eq 0)
$p.CloseMainWindow() | Out-Null
Check 'app exits after window close' ($p.WaitForExit(20000))

# --- 3. Normal close leaves nothing behind --------------------------------
Log ''
Log '3. Normal close leaves no service processes or listeners'
$p = Start-App
$r = Wait-Ready $p
Check 'relaunch reaches ready with health' ($r.Ready -and $r.Health)
$svcPids = @(Get-ServiceProcesses $p.Id | ForEach-Object { [int]$_.ProcessId })
$ports = @(Get-Listeners $svcPids | ForEach-Object { $_.LocalPort })
$p.CloseMainWindow() | Out-Null
Check 'app exits after window close' ($p.WaitForExit(20000))
Check 'no service process remains' (WaitUntil { @(Get-Alive $svcPids).Count -eq 0 } 10)
$left = @(Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $ports -contains $_.LocalPort })
Check 'no listener remains on the service port' ($left.Count -eq 0) (($ports) -join ', ')

# --- 4. Forced termination cleans up descendants --------------------------
Log ''
Log '4. Forced termination of the app cleans up its service'
$p = Start-App
$r = Wait-Ready $p
Check 'relaunch reaches ready with health' ($r.Ready -and $r.Health)
$svcPids = @(Get-ServiceProcesses $p.Id | ForEach-Object { [int]$_.ProcessId })
Stop-Process -Id $p.Id -Force
Check 'service process is terminated with the app (Job Object)' (WaitUntil { @(Get-Alive $svcPids).Count -eq 0 } 10)
} catch {
    Check "validation script error: $($_.Exception.Message)" $false
} finally {
    Stop-LeftoverApps
}

Log ''
$verdict = if ($script:failures -eq 0) { 'ALL CHECKS PASSED' } else { "$($script:failures) CHECK(S) FAILED" }
Log $verdict
$script:lines | Set-Content -Encoding UTF8 $ReportPath
Write-Host "Report: $ReportPath"
if ($script:failures -eq 0) { exit 0 } else { exit 1 }
