# Builds klm.exe in release mode and packs it with the KiCad plugin into a PCM-installable ZIP
# (KiCad: Plugin and Content Manager > Install from File...). Output: dist/klm-pcm-<version>.zip
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Push-Location $root
try {
    cargo build --release -p klm-gui
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    $version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
    $meta = Get-Content kicad_plugin/metadata.json -Raw | ConvertFrom-Json
    $meta.versions[0].version = $version

    # name in the archive -> file on disk. The archive layout is fixed by the PCM: plugins/, resources/, metadata.json.
    $files = [ordered]@{
        'plugins/plugin.json'      = 'kicad_plugin/plugin.json'
        'plugins/launch.py'        = 'kicad_plugin/launch.py'
        'plugins/requirements.txt' = 'kicad_plugin/requirements.txt'
        'plugins/klm-24.png'       = 'kicad_plugin/klm-24.png'
        'plugins/klm.exe'          = 'target/release/klm.exe'
        'resources/icon.png'       = 'kicad_plugin/icon-64.png'
    }
    $meta.versions[0] | Add-Member -NotePropertyName install_size -NotePropertyValue (($files.Values | ForEach-Object { (Get-Item $_).Length } | Measure-Object -Sum).Sum)

    New-Item -ItemType Directory -Force dist | Out-Null
    $zip = Join-Path $root "dist/klm-pcm-$version.zip"
    Remove-Item $zip -ErrorAction SilentlyContinue
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::Open($zip, 'Create')
    try {
        # Entry names are given explicitly: Windows PowerShell's Compress-Archive can write backslashes, which the PCM rejects.
        foreach ($name in $files.Keys) {
            [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, (Resolve-Path $files[$name]).Path, $name)
        }
        $entry = $archive.CreateEntry('metadata.json')
        $w = New-Object IO.StreamWriter($entry.Open(), (New-Object Text.UTF8Encoding($false)))
        $w.Write(($meta | ConvertTo-Json -Depth 10))
        $w.Dispose()
    } finally { $archive.Dispose() }

    $item = Get-Item $zip
    Write-Host "Wrote $zip"
    Write-Host "For a PCM repository entry: download_sha256 = $((Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()), download_size = $($item.Length)"
} finally { Pop-Location }
