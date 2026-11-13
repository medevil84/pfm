#!/usr/bin/env pwsh
<#
.SYNOPSIS
  CI script that verifies key parity between IT and EN i18n catalogs
  (both backend and frontend). Fails (exit code 1) if any key is missing
  in either direction.

.USAGE
  pwsh -NoProfile -File scripts/check_i18n_parity.ps1
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$catalogs = @(
    @{ Name = 'Backend';  It = "crates\pfm-i18n\catalogs\it.json"; En = "crates\pfm-i18n\catalogs\en.json" },
    @{ Name = 'Frontend'; It = "web\i18n\it.json";                  En = "web\i18n\en.json" }
)

$fail = $false

foreach ($cat in $catalogs) {
    $itPath = Join-Path $root $cat.It
    $enPath = Join-Path $root $cat.En

    if (-not (Test-Path $itPath)) {
        Write-Host "FAIL [$($cat.Name)] missing file: $($cat.It)" -ForegroundColor Red
        $fail = $true
        continue
    }
    if (-not (Test-Path $enPath)) {
        Write-Host "FAIL [$($cat.Name)] missing file: $($cat.En)" -ForegroundColor Red
        $fail = $true
        continue
    }

    $itKeys = (Get-Content $itPath -Raw -Encoding UTF8 | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $_.Name } | Sort-Object
    $enKeys = (Get-Content $enPath -Raw -Encoding UTF8 | ConvertFrom-Json).PSObject.Properties | ForEach-Object { $_.Name } | Sort-Object

    $missingInEn = $itKeys | Where-Object { $enKeys -notcontains $_ }
    $missingInIt = $enKeys | Where-Object { $itKeys -notcontains $_ }

    if ($missingInEn -or $missingInIt) {
        Write-Host "FAIL [$($cat.Name)] key mismatch detected:" -ForegroundColor Red
        if ($missingInEn) {
            Write-Host "  Missing in EN ($($missingInEn.Count)):" -ForegroundColor Yellow
            $missingInEn | ForEach-Object { Write-Host "    $_" -ForegroundColor Yellow }
        }
        if ($missingInIt) {
            Write-Host "  Missing in IT ($($missingInIt.Count)):" -ForegroundColor Yellow
            $missingInIt | ForEach-Object { Write-Host "    $_" -ForegroundColor Yellow }
        }
        $fail = $true
    }
    else {
        Write-Host "OK   [$($cat.Name)] $($itKeys.Count) keys match" -ForegroundColor Green
    }
}

if ($fail) {
    Write-Host ""
    Write-Host "i18n parity check FAILED" -ForegroundColor Red
    exit 1
}
else {
    Write-Host ""
    Write-Host "i18n parity check PASSED" -ForegroundColor Green
    exit 0
}