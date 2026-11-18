# Run the optional Model component proof without filling the source drive.
param(
    [string]$BuildDirectory = (Join-Path ([IO.Path]::GetTempPath()) 'intersect-model-component-proof'),
    [string]$ReportPath = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $ReportPath) {
    $ReportPath = Join-Path $repoRoot 'target/component-proof/report.json'
}
$proofBuildPath = [IO.Path]::GetFullPath($BuildDirectory)
$proofReportPath = [IO.Path]::GetFullPath($ReportPath)
$previousIncremental = $env:CARGO_INCREMENTAL
$proofExitCode = 1
Push-Location -LiteralPath $repoRoot
try {
    $env:CARGO_INCREMENTAL = '0'
    Write-Host "Build directory: $proofBuildPath"
    & cargo run --locked --profile test-fast --target-dir $proofBuildPath `
        -p intersect-model --features components --example model_component_usage `
        -- --out $proofReportPath
    $proofExitCode = $LASTEXITCODE
    if ($proofExitCode -eq 0) {
        $proofExecutable = Join-Path $repoRoot 'target/component-proof/model_component_usage.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $proofExecutable) | Out-Null
        Copy-Item -LiteralPath (Join-Path $proofBuildPath 'test-fast/examples/model_component_usage.exe') `
            -Destination $proofExecutable -Force
        Write-Host "Ready-to-run harness: $proofExecutable"
    }
}
finally {
    if ($null -eq $previousIncremental) { Remove-Item Env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue }
    else { $env:CARGO_INCREMENTAL = $previousIncremental }
    Pop-Location
}
exit $proofExitCode
