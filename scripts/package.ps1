# Builds Keeplight and produces, in dist/:
#   Keeplight-<version>-portable.zip
#   Keeplight-<version>-setup.exe   (when Inno Setup 6 is installed)
#   SHA256SUMS.txt
# Packaging happens in dist/staging; the repository root is never zipped.
param([switch] $SkipBuild, [switch] $SkipInstaller)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$dist = Join-Path $root 'dist'
$staging = Join-Path $dist 'staging'
$version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
    Where-Object name -eq 'keeplight' | Select-Object -ExpandProperty version

if (-not $SkipBuild) {
    cargo build --release --locked -p keeplight
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}

if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory -Force $staging | Out-Null
Copy-Item (Join-Path $root 'target\release\keeplight.exe') $staging
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $staging 'LICENSE.txt')
Copy-Item (Join-Path $root 'README.md') $staging

# Third-party notices: every non-workspace crate in the Windows build graph,
# with the license files shipped in its source package.
$meta = cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json
$workspace = @($meta.workspace_members)
$used = @{}
foreach ($node in $meta.resolve.nodes) { $used[$node.id] = $true }
$notices = New-Object System.Text.StringBuilder
[void]$notices.AppendLine('Keeplight includes the following third-party crates.')
[void]$notices.AppendLine('')
foreach ($pkg in ($meta.packages | Sort-Object name, version)) {
    if ($workspace -contains $pkg.id -or -not $used.ContainsKey($pkg.id)) { continue }
    [void]$notices.AppendLine(('=' * 78))
    [void]$notices.AppendLine("$($pkg.name) $($pkg.version) - $($pkg.license)")
    if ($pkg.repository) { [void]$notices.AppendLine($pkg.repository) }
    $dir = Split-Path $pkg.manifest_path
    $files = Get-ChildItem $dir -File | Where-Object { $_.Name -match '^(LICEN[SC]E|COPYING|NOTICE)' }
    foreach ($f in $files) {
        [void]$notices.AppendLine('')
        [void]$notices.AppendLine("--- $($f.Name)")
        [void]$notices.AppendLine((Get-Content $f.FullName -Raw))
    }
    [void]$notices.AppendLine('')
}
# Fonts embedded in the exe (assets/fonts).
[void]$notices.AppendLine(('=' * 78))
[void]$notices.AppendLine('Geist and Geist Mono fonts 1.802 - OFL-1.1')
[void]$notices.AppendLine('https://github.com/vercel/geist-font')
[void]$notices.AppendLine('')
[void]$notices.AppendLine('--- OFL.txt')
[void]$notices.AppendLine((Get-Content (Join-Path $root 'assets\fonts\OFL.txt') -Raw))
Set-Content -Path (Join-Path $staging 'THIRD-PARTY-LICENSES.txt') -Value $notices.ToString() -Encoding utf8

$zip = Join-Path $dist "Keeplight-$version-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path (Join-Path $staging '*') -DestinationPath $zip

# Verify the archive holds exactly what is expected and nothing private.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$entries = [System.IO.Compression.ZipFile]::OpenRead($zip).Entries | ForEach-Object FullName
$expected = 'LICENSE.txt', 'README.md', 'THIRD-PARTY-LICENSES.txt', 'keeplight.exe'
if (Compare-Object ($entries | Sort-Object) ($expected | Sort-Object)) {
    throw "Unexpected zip contents: $($entries -join ', ')"
}

$artifacts = @($zip)
if (-not $SkipInstaller) {
    $cmd = Get-Command iscc -ErrorAction SilentlyContinue
    $iscc = if ($cmd) { $cmd.Source } else { $null }
    foreach ($candidate in "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") {
        if (-not $iscc -and (Test-Path $candidate)) { $iscc = $candidate }
    }
    if ($iscc) {
        & $iscc /Q "/DAppVersion=$version" "/DSourceDir=$staging" "/DOutputDir=$dist" (Join-Path $root 'packaging\installer.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Inno Setup failed' }
        $artifacts += Join-Path $dist "Keeplight-$version-setup.exe"
    } else {
        Write-Warning 'Inno Setup 6 not found; skipped the installer.'
    }
}

$sums = foreach ($a in $artifacts) { '{0}  {1}' -f (Get-FileHash $a -Algorithm SHA256).Hash.ToLower(), (Split-Path $a -Leaf) }
Set-Content -Path (Join-Path $dist 'SHA256SUMS.txt') -Value $sums -Encoding ascii
Remove-Item -Recurse -Force $staging
$sums
