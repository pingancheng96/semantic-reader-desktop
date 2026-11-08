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
    $archive = Join-Path $buildDir 'sqlite.zip'
    Invoke-WebRequest -UseBasicParsing 'https://sqlite.org/2025/sqlite-amalgamation-3500400.zip' -OutFile $archive
    Expand-Archive -LiteralPath $archive -DestinationPath $buildDir -Force
    $env:SEMANTIC_READER_SQLITE_SOURCE = Join-Path $buildDir 'sqlite-amalgamation-3500400/sqlite3.c'
    $env:CARGO_TARGET_DIR = $buildDir

    $verify = "const {createHash}=require('node:crypto');const {readFileSync}=require('node:fs');if(createHash('sha3-256').update(readFileSync(process.env.SEMANTIC_READER_SQLITE_SOURCE)).digest('hex')!=='9145255e83da6529e70121ee4d7a4c88fe83ca4511da0c9ed13d10842df36782')throw Error('SQLite checksum mismatch')"
    Invoke-Checked node @('-e', $verify)
    Invoke-Checked npm.cmd @('ci')
    Invoke-Checked npm.cmd @('run', 'build')
    $cli = '@tauri-apps/cli@2.12.4'
    & curl.exe -fsIL --max-time 20 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.13/nsis-3.13.zip' | Out-Null
    if ($LASTEXITCODE -ne 0) {
        $cli = '@tauri-apps/cli@2.11.3'
        Invoke-Checked curl.exe @('-fsIL', '--max-time', '30', 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip')
    }
    Invoke-Checked npx.cmd @('--yes', $cli, 'build', '--target', 'x86_64-pc-windows-msvc', '--bundles', 'nsis', '--', '--locked')
    Get-ChildItem (Join-Path $buildDir 'x86_64-pc-windows-msvc/release/bundle/nsis') -Filter '*-setup.exe'
} finally {
    $env:SEMANTIC_READER_SQLITE_SOURCE = $previousSource
    $env:CARGO_TARGET_DIR = $previousTarget
    Pop-Location
}
