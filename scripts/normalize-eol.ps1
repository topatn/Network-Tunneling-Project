# Normalise tracked text files to the line endings .gitattributes declares.
# PowerShell's WriteAllLines/WriteAllText emit CRLF by default, which has repeatedly
# reintroduced CRLF into files that .gitattributes pins to LF. The audit catches it,
# but only after the edit; this fixes it before the commit.
#
#   pwsh -File scripts/normalize-eol.ps1

$ErrorActionPreference = 'Stop'
Set-Location -Path (Join-Path $PSScriptRoot '..')

# Fail loudly rather than reporting a clean bill of health on an empty file list.
# An earlier version did exactly that: a wrong Set-Location put it outside the
# repository, `git ls-files` failed, the loop iterated nothing, and it printed
# "all tracked text files already match .gitattributes" -- a false pass, which is
# precisely the failure mode this script exists to help avoid.
$tracked = @(git ls-files 2>$null)
if ($LASTEXITCODE -ne 0 -or $tracked.Count -eq 0) {
    Write-Error "git ls-files failed or returned nothing from $(Get-Location). Refusing to report success."
    exit 1
}

# Types .gitattributes declares as `text eol=lf`.
$lfTypes = @('.py', '.yml', '.yaml', '.md', '.toml', '.json', '.sql', '.rs')
# Types .gitattributes declares as `text eol=crlf`.
$crlfTypes = @('.bat', '.cmd', '.ps1')
$lfNames = @('.gitattributes', '.gitignore')

$changed = @()
foreach ($f in $tracked) {
    $name = Split-Path $f -Leaf
    $ext = [System.IO.Path]::GetExtension($f)
    $want = $null
    if ($lfNames -contains $name) { $want = "`n" }
    elseif ($lfTypes -contains $ext) { $want = "`n" }
    elseif ($crlfTypes -contains $ext) { $want = "`r`n" }
    if ($null -eq $want) { continue }
    if (-not (Test-Path $f)) { continue }

    $text = [System.IO.File]::ReadAllText((Resolve-Path $f))
    $text = $text.Replace("`r`n", "`n")
    if ($want -eq "`r`n") { $text = $text.Replace("`n", "`r`n") }

    $bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $f))
    $hasCrLf = $false
    $hasLf = $false
    for ($i = 0; $i -lt $bytes.Length; $i++) {
        if ($bytes[$i] -eq 10) {
            if ($i -gt 0 -and $bytes[$i - 1] -eq 13) { $hasCrLf = $true } else { $hasLf = $true }
        }
    }
    $needsLf = ($want -eq "`n" -and $hasCrLf)
    $needsCrLf = ($want -eq "`r`n" -and $hasLf -and -not $hasCrLf)
    if ($needsLf -or $needsCrLf) {
        [System.IO.File]::WriteAllText((Resolve-Path $f), $text,
            (New-Object System.Text.UTF8Encoding($false)))
        $changed += $f
    }
}

if ($changed.Count -eq 0) {
    Write-Host "all tracked text files already match .gitattributes"
}
else {
    Write-Host "normalised $($changed.Count) file(s):"
    $changed | ForEach-Object { Write-Host "  $_" }
}
