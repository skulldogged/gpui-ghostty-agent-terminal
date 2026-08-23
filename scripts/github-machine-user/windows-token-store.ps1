param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet("store", "lookup")]
    [string]$Command,

    [Parameter(Mandatory = $true, Position = 1)]
    [string]$TokenFile
)

$ErrorActionPreference = "Stop"
$securityAssembly = [System.Reflection.Assembly]::LoadWithPartialName("System.Security")
if ($null -eq $securityAssembly) {
    throw "Windows DPAPI assembly System.Security could not be loaded"
}
$entropy = [Text.Encoding]::UTF8.GetBytes("codex-github-machine-user-v1")

switch ($Command) {
    "store" {
        $plainText = [Console]::In.ReadToEnd().TrimEnd("`r", "`n")
        if ([string]::IsNullOrWhiteSpace($plainText)) {
            throw "refusing to store an empty token"
        }

        $directory = Split-Path -Parent $TokenFile
        New-Item -ItemType Directory -Force -Path $directory | Out-Null
        $plainBytes = [Text.Encoding]::UTF8.GetBytes($plainText)
        try {
            $protectedBytes = [System.Security.Cryptography.ProtectedData]::Protect(
                $plainBytes,
                $entropy,
                [System.Security.Cryptography.DataProtectionScope]::CurrentUser
            )
            [IO.File]::WriteAllBytes($TokenFile, $protectedBytes)
        }
        finally {
            [Array]::Clear($plainBytes, 0, $plainBytes.Length)
            $plainText = $null
        }
    }
    "lookup" {
        if (-not (Test-Path -LiteralPath $TokenFile -PathType Leaf)) {
            throw "token file does not exist: $TokenFile"
        }

        $protectedBytes = [IO.File]::ReadAllBytes($TokenFile)
        $plainBytes = [System.Security.Cryptography.ProtectedData]::Unprotect(
            $protectedBytes,
            $entropy,
            [System.Security.Cryptography.DataProtectionScope]::CurrentUser
        )
        try {
            $plainText = [Text.Encoding]::UTF8.GetString($plainBytes)
            if ([string]::IsNullOrWhiteSpace($plainText)) {
                throw "stored token is empty"
            }
            [Console]::Out.Write($plainText)
        }
        finally {
            [Array]::Clear($plainBytes, 0, $plainBytes.Length)
            $plainText = $null
        }
    }
}
