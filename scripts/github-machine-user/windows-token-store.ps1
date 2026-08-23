param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet("store", "lookup")]
    [string]$Command,

    [Parameter(Mandatory = $true, Position = 1)]
    [string]$TokenFile
)

$ErrorActionPreference = "Stop"

function Clear-PlainText([ref]$Value) {
    $Value.Value = $null
    [GC]::Collect()
}

switch ($Command) {
    "store" {
        $plainText = [Console]::In.ReadToEnd().TrimEnd("`r", "`n")
        if ([string]::IsNullOrWhiteSpace($plainText)) {
            throw "refusing to store an empty token"
        }

        $directory = Split-Path -Parent $TokenFile
        New-Item -ItemType Directory -Force -Path $directory | Out-Null
        $secure = ConvertTo-SecureString $plainText -AsPlainText -Force
        $credential = [PSCredential]::new("github-machine-user", $secure)
        $credential | Export-Clixml -LiteralPath $TokenFile -Force
        Clear-PlainText ([ref]$plainText)
    }
    "lookup" {
        if (-not (Test-Path -LiteralPath $TokenFile -PathType Leaf)) {
            throw "token file does not exist: $TokenFile"
        }

        $credential = Import-Clixml -LiteralPath $TokenFile
        if ($credential -isnot [PSCredential]) {
            throw "token file does not contain a Windows credential"
        }

        $plainText = $credential.GetNetworkCredential().Password
        if ([string]::IsNullOrWhiteSpace($plainText)) {
            throw "stored token is empty"
        }
        [Console]::Out.Write($plainText)
        Clear-PlainText ([ref]$plainText)
    }
}
