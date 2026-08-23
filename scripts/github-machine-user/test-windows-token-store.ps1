$ErrorActionPreference = "Stop"

$helper = Join-Path $PSScriptRoot "windows-token-store.ps1"
$testRoot = Join-Path ([IO.Path]::GetTempPath()) (
    "codex-windows-token-store-test-{0}" -f [Guid]::NewGuid().ToString("N")
)
$tokenFile = Join-Path $testRoot "github-token.bin"
$fixture = "fixture-token-{0}" -f [Guid]::NewGuid().ToString("N")

try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null
    $fixture | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $helper store $tokenFile
    if ($LASTEXITCODE -ne 0) {
        throw "token storage helper failed with exit code $LASTEXITCODE"
    }

    $actual = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $helper lookup $tokenFile
    if ($LASTEXITCODE -ne 0) {
        throw "token lookup helper failed with exit code $LASTEXITCODE"
    }
    if ($actual -ne $fixture) {
        throw "DPAPI token round trip returned a different value"
    }

    $storedText = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($tokenFile))
    if ($storedText.Contains($fixture)) {
        throw "the stored credential contains the plaintext fixture"
    }

    Write-Host "Windows DPAPI token-store test passed"
}
finally {
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
