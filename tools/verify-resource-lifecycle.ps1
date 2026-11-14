# Repository verification only: no installs, source edits, commits or publication.
[CmdletBinding()]
param(
    [ValidateSet('focused','preflight','native','native-tail','wasm','browser','all')]
    [string]$Lane = 'focused',
    [string]$TargetDirectory,
    [string]$BrowserTargetDirectory,
    [string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
$commonGit = (& git -C $repository rev-parse --path-format=absolute --git-common-dir).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot identify repository workspace' }
$workspace = Split-Path -Parent $commonGit
function WorkspacePath([string]$value) {
    $absolute = [System.IO.Path]::GetFullPath($value)
    if (-not $absolute.StartsWith($workspace.TrimEnd('\','/') + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Verification artifacts must stay inside the repository workspace: $absolute"
    }
    return $absolute
}
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $repository 'target' }
if (-not $BrowserTargetDirectory) { $BrowserTargetDirectory = $TargetDirectory }
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repository ('target/resource-verification/' + $Lane + '-' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
$TargetDirectory = WorkspacePath $TargetDirectory
$BrowserTargetDirectory = WorkspacePath $BrowserTargetDirectory
$OutputDirectory = WorkspacePath $OutputDirectory
New-Item -ItemType Directory -Path $OutputDirectory -ErrorAction Stop | Out-Null
$env:CARGO_TARGET_DIR = $TargetDirectory
$env:CARGO_PROFILE_DEV_DEBUG = '0'
$env:CARGO_PROFILE_TEST_DEBUG = '0'
$env:CARGO_PROFILE_TEST_OPT_LEVEL = '1'
$env:CARGO_INCREMENTAL = '0'
$env:RUST_MIN_STACK = '67108864'
$env:CARGO_BUILD_JOBS = '2'
$checks = [System.Collections.Generic.List[object]]::new()
$base = (& git -C $repository rev-parse HEAD).Trim()
function Check([string]$Name, [string]$Program, [string[]]$Arguments, [string]$Directory = $repository, [switch]$KeepGoing) {
    $log = Join-Path $OutputDirectory ($Name + '.log')
    Write-Output "Running $Name"
    Push-Location $Directory
    $started = [DateTime]::UtcNow
    try {
        $ErrorActionPreference = 'Continue'
        & $Program @Arguments 2>&1 | ForEach-Object { $_.ToString() } | Tee-Object -FilePath $log
        $code = $LASTEXITCODE
    } finally { Pop-Location; $ErrorActionPreference = 'Stop' }
    $checks.Add([ordered]@{name=$Name;program=$Program;arguments=$Arguments;directory=$Directory;exit_code=$code;elapsed_seconds=([DateTime]::UtcNow-$started).TotalSeconds;log=$log})
    if ($code -ne 0 -and -not $KeepGoing) { throw "$Name failed with exit code $code; see $log" }
}
try {
    if ($Lane -in @('preflight','all')) {
        Check 'format' 'python' @('-B','tools/format_workspace.py','--check')
        Check 'clippy' 'cargo' @('clippy','--workspace','--all-targets','--locked','--offline','--','-D','warnings')
    }
    if ($Lane -in @('focused','all')) {
        Check 'portable-native' 'cargo' @('test','-p','intersect-resource-lifecycle-tests','--locked','--offline')
        Check 'sqlite-xmi' 'cargo' @('test','-p','intersect-acceptance','--test','sqlite_resource_set','--test','xmi_interchange','--locked','--offline')
        Check 'json-adapter' 'cargo' @('test','-p','intersect-web','--test','resource_lifecycle','--locked','--offline')
        Check 'project-node' 'node' @('--test','toolkit/tests/entry-project.node.js','toolkit/tests/inference-project.node.js')
    }
    if ($Lane -in @('native','all')) {
        Check 'workspace-tests' 'cargo' @('test','--workspace','--no-fail-fast','--locked','--offline') -KeepGoing
    }
    if ($Lane -in @('native','native-tail','all')) {
        Check 'library-example' 'cargo' @('run','-p','intersect-model','--example','library','--locked','--offline')
        Check 'model-example' 'cargo' @('run','-p','intersect-acceptance','--example','model_api','--locked','--offline')
        Check 'bindings-example' 'cargo' @('run','-p','intersect-rust-bindings-tests','--example','generated_library','--locked','--offline')
        Check 'cache-measurement' 'cargo' @('run','-p','intersect-resource-lifecycle-tests','--example','prepared_library','--locked','--offline')
        Check 'model-boundary' 'python' @('-B','tools/check_model_boundary.py')
        Check 'model-only-format' 'cargo' @('fmt','--manifest-path','tests/model-only/Cargo.toml','--','--check')
        Check 'model-only-clippy' 'cargo' @('clippy','--manifest-path','tests/model-only/Cargo.toml','--target-dir',(Join-Path $repository 'target/model-only'),'--all-targets','--locked','--offline','--','-D','warnings')
        Check 'model-only-tests' 'cargo' @('test','--manifest-path','tests/model-only/Cargo.toml','--target-dir',(Join-Path $repository 'target/model-only'),'--locked','--offline')
        Check 'definition-ledger' 'python' @('-B','tools/definition_ledger.py','--check')
        $pythonTools = Join-Path $repository 'tools'
        if ($env:OS -eq 'Windows_NT') { $pythonTools = '\\?\' + $pythonTools }
        Check 'python-tools' 'python' @('-B','-m','unittest','discover','-s',$pythonTools,'-p','test_*.py')
    }
    if ($Lane -in @('wasm','all')) {
        Check 'workspace-wasm' 'cargo' @('build','--workspace','--lib','--target','wasm32-unknown-unknown','--locked','--offline')
        Check 'portable-wasm' 'node' @('tools/test_resource_lifecycle_wasm.mjs',(Join-Path $TargetDirectory 'wasm32-unknown-unknown/debug/intersect_resource_lifecycle_tests.wasm'))
        Check 'interchange-wasm' 'node' @('tools/test_resource_interchange_wasm.mjs',(Join-Path $TargetDirectory 'wasm32-unknown-unknown/debug/intersect_web.wasm'))
        Check 'model-only-wasm-build' 'cargo' @('build','--manifest-path','tests/model-only/Cargo.toml','--target-dir',(Join-Path $repository 'target/model-only'),'--lib','--target','wasm32-unknown-unknown','--locked','--offline')
        Check 'model-only-wasm-run' 'node' @('tools/test_model_boundary_wasm.mjs')
    }
    if ($Lane -in @('browser','all')) {
        $env:CARGO_TARGET_DIR = $BrowserTargetDirectory
        $toolkit = Join-Path $repository 'toolkit'
        Check 'toolkit-build' 'npm.cmd' @('run','build') $toolkit
        $env:INTERSECT_TEST_PORT = '4297'
        Check 'browser-development' 'node' @('node_modules/@playwright/test/cli.js','test','tests/resource-lifecycle.spec.js','tests/interchange.spec.js','--reporter=line') $toolkit
        $env:INTERSECT_TEST_PORT = '4298'
        Check 'browser-production' 'node' @('node_modules/@playwright/test/cli.js','test','--config','playwright.production.config.js','tests/resource-lifecycle.spec.js','--reporter=line') $toolkit
    }
} finally {
    [ordered]@{lane=$Lane;source_base=$base;repository=$repository;target=$TargetDirectory;browser_target=$BrowserTargetDirectory;checks=$checks.ToArray()} |
        ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'receipt.json') -Encoding UTF8
}
if (@($checks | Where-Object { $_.exit_code -ne 0 }).Count -gt 0) {
    throw "Verification checks failed; see $OutputDirectory/receipt.json"
}
