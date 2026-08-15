#Requires -Version 5.1
$ErrorActionPreference = "Stop"

$Repo = "dariocurr/shaic"
$Prefix = if ($env:PREFIX) { $env:PREFIX } else { Join-Path $env:USERPROFILE ".local" }
$BinDir = Join-Path $Prefix "bin"

$arch = $env:PROCESSOR_ARCHITECTURE
$target = switch ($arch) {
    "AMD64" { "x86_64-pc-windows-msvc" }
    "ARM64" { "aarch64-pc-windows-msvc" }
    default {
        Write-Error "unsupported Windows arch: $arch"
        exit 1
    }
}

$asset = "shaic-$target.zip"
$sums = "SHA256SUMS"
$base = "https://github.com/$Repo/releases/latest/download"
$tmpdir = Join-Path ([IO.Path]::GetTempPath()) ("shaic-install-" + [guid]::NewGuid().ToString("n"))
New-Item -ItemType Directory -Path $tmpdir | Out-Null

try {
    Write-Host "downloading $asset..."
    Invoke-WebRequest -Uri "$base/$asset" -OutFile (Join-Path $tmpdir $asset)

    $checksumFile = Join-Path $tmpdir "$asset.sha256"
    $expected = $null
    try {
        Invoke-WebRequest -Uri "$base/$asset.sha256" -OutFile $checksumFile
        $expected = (Get-Content $checksumFile -Raw).Trim().Split()[0]
    } catch {
        Invoke-WebRequest -Uri "$base/$sums" -OutFile (Join-Path $tmpdir $sums)
        $expected = (Select-String -Path (Join-Path $tmpdir $sums) -Pattern " $([regex]::Escape($asset))$").Line.Split()[0]
    }
    $assetPath = Join-Path $tmpdir $asset
    $actual = (Get-FileHash $assetPath -Algorithm SHA256).Hash
    if ($expected.ToUpper() -ne $actual.ToUpper()) {
        throw "checksum mismatch for $asset"
    }
    Expand-Archive -Path (Join-Path $tmpdir $asset) -DestinationPath $tmpdir -Force

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item (Join-Path $tmpdir "shaic.exe") (Join-Path $BinDir "shaic.exe") -Force
    Write-Host "installed $(Join-Path $BinDir 'shaic.exe')"
    Write-Host "put $BinDir on PATH if it is not already"
    & (Join-Path $BinDir "shaic.exe") --version
}
finally {
    Remove-Item -Recurse -Force $tmpdir -ErrorAction SilentlyContinue
}
