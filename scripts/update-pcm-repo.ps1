# Adds a released package zip to the KiCad PCM custom repository in pcm/ (packages.json, then repository.json).
# Run by the release workflow; also fine by hand: scripts/update-pcm-repo.ps1 -Version 0.10.0 -ZipPath dist/klm-pcm-0.10.0.zip
# -PcmDir exists so the script can be tried on a copy. Older versions already in packages.json are kept.
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$ZipPath,
    [string]$PcmDir = (Join-Path (Split-Path $PSScriptRoot) 'pcm'),
    [string]$Repo = 'Jconnor20/KiCAD-Library-Manager'
)
$ErrorActionPreference = 'Stop'

# LF and no BOM: repository.json's sha256 must match the bytes git stores (see .gitattributes).
function Write-Json($path, $obj) {
    $text = (($obj | ConvertTo-Json -Depth 10) -replace "`r`n", "`n") + "`n"
    [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding($false)))
}

$zip = (Resolve-Path $ZipPath).Path
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead($zip)
try {
    $r = New-Object IO.StreamReader($archive.GetEntry('metadata.json').Open())
    $meta = $r.ReadToEnd() | ConvertFrom-Json
    $r.Dispose()
} finally { $archive.Dispose() }

$entry = $meta.versions[0]
if ($entry.version -ne $Version) { throw "metadata.json in the zip says $($entry.version), expected $Version" }
$entry | Add-Member -NotePropertyName download_url -NotePropertyValue "https://github.com/$Repo/releases/download/v$Version/$(Split-Path $zip -Leaf)"
$entry | Add-Member -NotePropertyName download_sha256 -NotePropertyValue (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
$entry | Add-Member -NotePropertyName download_size -NotePropertyValue (Get-Item $zip).Length

$packagesPath = Join-Path $PcmDir 'packages.json'
$old = (Get-Content $packagesPath -Raw | ConvertFrom-Json).packages | Where-Object { $_.identifier -eq $meta.identifier }
$kept = @($old.versions | Where-Object { $_ -and $_.version -ne $Version })
# Package fields come from the zip's metadata so description edits reach the repository too.
$meta.versions = @($kept + $entry)
Write-Json $packagesPath ([ordered]@{ packages = @($meta) })

$now = [DateTimeOffset]::UtcNow
Write-Json (Join-Path $PcmDir 'repository.json') ([ordered]@{
    '$schema'  = 'https://go.kicad.org/pcm/schemas/v2#/definitions/Repository'
    name       = 'KiCad Library Manager'
    maintainer = [ordered]@{ name = 'Jconnor20'; contact = [ordered]@{ web = "https://github.com/$Repo" } }
    packages   = [ordered]@{
        url             = "https://raw.githubusercontent.com/$Repo/master/pcm/packages.json"
        sha256          = (Get-FileHash $packagesPath -Algorithm SHA256).Hash.ToLower()
        update_time_utc = $now.ToString('yyyy-MM-dd HH:mm:ss')
        update_timestamp = $now.ToUnixTimeSeconds()
    }
})
Write-Host "PCM repository updated for v$Version ($($entry.download_size) bytes, sha256 $($entry.download_sha256))"
