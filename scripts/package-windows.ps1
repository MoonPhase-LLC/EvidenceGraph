# S1-09: build the Windows installer with the bundled service.
# See docs/PACKAGING.md. Run from anywhere:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package-windows.ps1
#   ... -ServiceOnly   # steps 1-2 only (used by CI)
#
# Steps:
#   1. Freeze the service with PyInstaller (--onedir) from an isolated build
#      environment holding only the locked runtime dependencies + PyInstaller.
#   2. Check the frozen bundle for build/dev-only packages.
#   3. Build the release app and NSIS installer with the bundle as a resource.
#
# Every native command's exit code is checked immediately: PowerShell does
# not stop on a failing native command by itself.

param([switch]$ServiceOnly)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Native {
    param([string]$Description, [scriptblock]$Command)
    Write-Host "==> $Description"
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$Description failed (exit code $LASTEXITCODE)" }
}

$repo = Split-Path -Parent $PSScriptRoot
$service = Join-Path $repo 'service'
$app = Join-Path $repo 'app'
$buildVenv = Join-Path $service 'build\package-venv'
$workPath = Join-Path $service 'build\pyinstaller'
$distPath = Join-Path $service 'dist\pyinstaller'
$bundle = Join-Path $distPath 'evidencegraph-service'

# 1. Freeze the service. A dedicated environment (not service/.venv) so the
#    bundle cannot pick up dev tools such as mypy or pytest.
foreach ($old in $workPath, $distPath) {
    if (Test-Path $old) { Remove-Item $old -Recurse -Force }  # throws if in use
}
$env:UV_PROJECT_ENVIRONMENT = $buildVenv
try {
    Push-Location $service
    Invoke-Native 'Create the isolated build environment (frozen, no dev group)' {
        uv sync --frozen --no-dev --group package --no-editable
    }
} finally {
    Pop-Location
    Remove-Item Env:UV_PROJECT_ENVIRONMENT
}
$python = Join-Path $buildVenv 'Scripts\python.exe'
Invoke-Native 'Show the bundled Python version' { & $python --version }

# Modules PyInstaller would otherwise collect that the service never uses at
# runtime: pydantic's optional mypy plugin, and PyInstaller's own setuptools
# dependency.
$excludes = @('mypy', 'pydantic.mypy', 'setuptools', 'pkg_resources', '_distutils_hack', 'pip')
$pyiArgs = @(
    '-m', 'PyInstaller', '--noconfirm', '--clean', '--onedir', '--console', '--noupx',
    '--name', 'evidencegraph-service',
    '--distpath', $distPath, '--workpath', $workPath, '--specpath', $workPath
)
foreach ($m in $excludes) { $pyiArgs += @('--exclude-module', $m) }
$pyiArgs += (Join-Path $service 'packaging\pyinstaller_entry.py')
Invoke-Native 'Freeze the service (PyInstaller --onedir)' { & $python @pyiArgs }

# 2. Inspect the bundle.
$exe = Join-Path $bundle 'evidencegraph-service.exe'
if (-not (Test-Path $exe -PathType Leaf)) { throw "frozen service not found at $exe" }
$forbidden = @('mypy', 'setuptools', 'pip', 'pytest', '_pytest', 'httpx', 'ruff', 'PyInstaller', 'tests')
$found = Get-ChildItem (Join-Path $bundle '_internal') -Directory |
    Where-Object { $forbidden -contains $_.Name -or $_.Name -like 'pytest*' -or $_.Name -like 'mypy*' }
if ($found) { throw "bundle contains build/dev-only packages: $($found.Name -join ', ')" }
# Pure-Python modules live in the PYZ archive inside the executable.
$archive = & $python -m PyInstaller.utils.cliutils.archive_viewer --list --recursive --brief $exe
if ($LASTEXITCODE -ne 0) { throw "could not list the frozen archive (exit code $LASTEXITCODE)" }
$pattern = '^\s*(' + (($forbidden | ForEach-Object { [regex]::Escape($_) }) -join '|') + ')(\.|$)'
$bad = @($archive | Where-Object { $_ -match $pattern })
if ($bad.Count -gt 0) { throw "frozen archive contains build/dev-only modules: $(($bad | Select-Object -First 10) -join ', ')" }
if (-not ($archive | Where-Object { $_ -match '^\s*evidencegraph_service\.supervised$' })) {
    throw 'frozen archive is missing evidencegraph_service.supervised'
}
Write-Host "Frozen service OK: $exe"
if ($ServiceOnly) { return }

# 3. Release app + NSIS installer. tauri.package.conf.json adds the bundle as
#    a resource (<install dir>/service/) and selects the NSIS target.
try {
    Push-Location $app
    Invoke-Native 'Install frontend dependencies (frozen)' { npm ci }
    Invoke-Native 'Build the release app and NSIS installer' {
        npm run tauri build -- --config src-tauri/tauri.package.conf.json
    }
} finally {
    Pop-Location
}

$installers = @(Get-ChildItem (Join-Path $app 'src-tauri\target\release\bundle\nsis') -Filter '*-setup.exe')
if ($installers.Count -ne 1) { throw "expected exactly one NSIS installer, found $($installers.Count)" }
$installer = $installers[0]
$hash = (Get-FileHash $installer.FullName -Algorithm SHA256).Hash
Write-Host ''
Write-Host "Installer: $($installer.FullName)"
Write-Host "Size:      $($installer.Length) bytes"
Write-Host "SHA-256:   $hash"
