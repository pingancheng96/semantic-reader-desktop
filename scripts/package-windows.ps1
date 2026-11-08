param([ValidateSet('All', 'Compile', 'Bundle')][string]$Phase = 'All')

$ErrorActionPreference = 'Stop'

function Invoke-Checked($Command, $Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Command failed with exit code $LASTEXITCODE" }
}

Push-Location (Join-Path $PSScriptRoot '..')
$previousSource = $env:SEMANTIC_READER_SQLITE_SOURCE
$previousTarget = $env:CARGO_TARGET_DIR
try {
    $buildDir = Join-Path $PWD 'src-tauri/target/windows-package'
    New-Item -ItemType Directory -Force $buildDir | Out-Null
    $env:CARGO_TARGET_DIR = $buildDir
    $archive = Join-Path $buildDir 'sqlite.zip'
    if ($Phase -ne 'Bundle') {
        $sqliteSource = Join-Path $buildDir 'sqlite-amalgamation-3500400/sqlite3.c'
        if (!(Test-Path -LiteralPath $sqliteSource)) {
            Invoke-WebRequest -UseBasicParsing 'https://sqlite.org/2025/sqlite-amalgamation-3500400.zip' -OutFile $archive
            Expand-Archive -LiteralPath $archive -DestinationPath $buildDir -Force
        }
        $env:SEMANTIC_READER_SQLITE_SOURCE = $sqliteSource

        $verify = "const {createHash}=require('node:crypto');const {readFileSync}=require('node:fs');if(createHash('sha3-256').update(readFileSync(process.env.SEMANTIC_READER_SQLITE_SOURCE)).digest('hex')!=='9145255e83da6529e70121ee4d7a4c88fe83ca4511da0c9ed13d10842df36782')throw Error('SQLite checksum mismatch')"
        Invoke-Checked node @('-e', $verify)
        Invoke-Checked npm.cmd @('ci')
        Invoke-Checked npm.cmd @('run', 'build')
        Invoke-Checked npx.cmd @('--yes', '@tauri-apps/cli@2.12.4', 'build', '--target', 'x86_64-pc-windows-msvc', '--no-bundle', '--', '--locked')
    }
    if ($Phase -eq 'Compile') { return }
    $nsisArchive = Join-Path $buildDir 'nsis.zip'
    $nsisPlugin = Join-Path $buildDir 'nsis_tauri_utils.dll'
    $assets = @(
        @{ Url = 'https://api.github.com/repos/tauri-apps/binary-releases/releases/assets/623832696?download=1'; Path = $nsisArchive; Sha1 = 'DB14F8AF2EA346B786C6AE19343F61AAF5E09B8E' },
        @{ Url = 'https://api.github.com/repos/tauri-apps/nsis-tauri-utils/releases/assets/345882097?download=1'; Path = $nsisPlugin; Sha1 = '75197FEE3C6A814FE035788D1C34EAD39349B860' }
    )
    foreach ($asset in $assets) {
        if (!(Test-Path -LiteralPath $asset.Path) -or (Get-FileHash -LiteralPath $asset.Path -Algorithm SHA1).Hash -ne $asset.Sha1) {
            Invoke-Checked curl.exe @('-fsSL', '--retry', '2', '--max-time', '60', '-H', 'Accept: application/octet-stream', $asset.Url, '-o', $asset.Path)
        }
        if ((Get-FileHash -LiteralPath $asset.Path -Algorithm SHA1).Hash -ne $asset.Sha1) {
            throw "NSIS download checksum mismatch: $($asset.Path)"
        }
    }
    Expand-Archive -LiteralPath $nsisArchive -DestinationPath $buildDir -Force
    $tools = Join-Path $env:LOCALAPPDATA 'tauri/NSIS'
    New-Item -ItemType Directory -Force $tools | Out-Null
    Copy-Item (Join-Path $buildDir 'nsis-3.13/*') $tools -Recurse -Force
    $pluginDir = Join-Path $tools 'Plugins/x86-unicode/additional'
    New-Item -ItemType Directory -Force $pluginDir | Out-Null
    Copy-Item $nsisPlugin $pluginDir -Force
    Invoke-Checked npx.cmd @('--yes', '@tauri-apps/cli@2.12.4', 'bundle', '--target', 'x86_64-pc-windows-msvc', '--bundles', 'nsis')
    Get-ChildItem (Join-Path $buildDir 'x86_64-pc-windows-msvc/release/bundle/nsis') -Filter '*-setup.exe'
} finally {
    $env:SEMANTIC_READER_SQLITE_SOURCE = $previousSource
    $env:CARGO_TARGET_DIR = $previousTarget
    Pop-Location
}
