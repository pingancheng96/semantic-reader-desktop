$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $root
Start-Transcript -Path (Join-Path $root 'build-windows.log') -Force
try {
    function Refresh-Path {
        $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' +
            [Environment]::GetEnvironmentVariable('Path', 'User') + ';' +
            (Join-Path $env:USERPROFILE '.cargo\bin')
    }
    function Checked($Command, $Arguments) {
        & $Command @Arguments
        if ($LASTEXITCODE -ne 0) { throw "$Command failed ($LASTEXITCODE)" }
    }
    function Find-CppTools {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (Test-Path -LiteralPath $vswhere) {
            & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        }
    }
    Refresh-Path
    $missing = @()
    if (!(Get-Command node.exe -ErrorAction SilentlyContinue)) { $missing += 'OpenJS.NodeJS.LTS' }
    if (!(Get-Command rustup.exe -ErrorAction SilentlyContinue)) { $missing += 'Rustlang.Rustup' }
    if (!(Find-CppTools)) { $missing += 'Microsoft.VisualStudio.2022.BuildTools' }
    if ($missing.Count) {
        Write-Host 'Required build tools are missing:'
        $missing | ForEach-Object { Write-Host "  $_" }
        Write-Host 'Installation downloads several GB and may request administrator approval.'
        if ((Read-Host 'Install these tools now? Type Y to continue') -ne 'Y') { throw 'Setup cancelled.' }
        if (!(Get-Command winget.exe -ErrorAction SilentlyContinue)) {
            throw 'Install App Installer from Microsoft Store, then run Build-Windows.cmd again.'
        }
        foreach ($package in $missing) {
            $arguments = @('install', '--exact', '--id', $package, '--source', 'winget')
            if ($package -eq 'Microsoft.VisualStudio.2022.BuildTools') {
                $arguments += @('--override', '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended')
            }
            Checked winget.exe $arguments
        }
        Refresh-Path
    }
    foreach ($command in @('node.exe', 'npm.cmd', 'rustup.exe', 'cargo.exe')) {
        if (!(Get-Command $command -ErrorAction SilentlyContinue)) {
            throw "$command is unavailable. Restart Windows after installation and run this script again."
        }
    }
    $vs = Find-CppTools
    if (!$vs) { throw 'Install Visual Studio C++ Build Tools with the Windows SDK, then retry.' }
    Checked rustup.exe @('target', 'add', 'x86_64-pc-windows-msvc')
    $developerShell = Join-Path $vs 'Common7\Tools\Launch-VsDevShell.ps1'
    if (!(Test-Path -LiteralPath $developerShell)) { throw 'Visual Studio developer shell is missing.' }
    & $developerShell -Arch amd64 -HostArch amd64 -SkipAutomaticLocation
    Set-Location -LiteralPath $root
    & (Join-Path $PSScriptRoot 'package-windows.ps1')
    $installers = @(Get-ChildItem (Join-Path $root 'src-tauri\target\windows-package\x86_64-pc-windows-msvc\release\bundle\nsis') -Filter '*-setup.exe')
    if (!$installers.Count) { throw 'No Windows installer was generated.' }
    $output = Join-Path $root 'Windows-output'
    New-Item -ItemType Directory -Force $output | Out-Null
    foreach ($installer in $installers) { Copy-Item -LiteralPath $installer.FullName -Destination $output -Force }
    Write-Host "Build complete. Installer saved in $output"
    Invoke-Item -LiteralPath $output
} catch {
    Write-Host $_ -ForegroundColor Red
    exit 1
} finally {
    Stop-Transcript
}
