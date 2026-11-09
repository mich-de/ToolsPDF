# Pubblica una nuova versione di Tools PDF su GitHub.
# Prima alzare "version" in rust/Cargo.toml, fare commit e push. Poi:
#   powershell -File release.ps1 -Notes "cosa cambia"
# Esegue i test, compila, scrive version.txt (versione + SHA-256 dell'exe) e crea la release vX.Y.Z.
# Le barre gia' installate leggono version.txt dall'ultima release e propongono l'aggiornamento.
param([string]$Notes = "")
$ErrorActionPreference = "Stop"

$repo = "mich-de/ToolsPDF"
$root = $PSScriptRoot
$rust = Join-Path $root "rust"
$match = Select-String -Path (Join-Path $rust "Cargo.toml") -Pattern '^version = "(.+)"' | Select-Object -First 1
$version = $match.Matches[0].Groups[1].Value

$llvm = Get-ChildItem (Join-Path $env:USERPROFILE ".llvm-mingw") -Directory | Select-Object -First 1
$env:PATH = (Join-Path $env:USERPROFILE ".cargo\bin") + ";" + (Join-Path $llvm.FullName "bin") + ";" + $env:PATH

if (git -C $root status --porcelain) { throw "Ci sono modifiche non salvate in git: fare commit prima della release." }
gh release view "v$version" --repo $repo 2>$null | Out-Null
if ($LASTEXITCODE -eq 0) { throw "La release v$version esiste gia': alzare la versione in rust/Cargo.toml." }

Push-Location $rust
try {
    cargo test
    if ($LASTEXITCODE -ne 0) { throw "Test falliti." }
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "Compilazione fallita." }
} finally {
    Pop-Location
}

$built = Join-Path $rust "target\x86_64-pc-windows-gnullvm\release\toolspdf.exe"
$out = Join-Path $rust "target\release-assets"
New-Item -ItemType Directory -Force $out | Out-Null
$exe = Join-Path $out "ToolsPDF.exe"
Copy-Item $built $exe -Force
Copy-Item $built (Join-Path $root "ToolsPDF.exe") -Force
$hash = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
$versionFile = Join-Path $out "version.txt"
[IO.File]::WriteAllText($versionFile, "$version`r`n$hash`r`n")

if (-not $Notes) { $Notes = "Tools PDF $version" }
git -C $root push
gh release create "v$version" $exe $versionFile --repo $repo --title "Tools PDF $version" --notes $Notes
if ($LASTEXITCODE -ne 0) { throw "Creazione della release non riuscita." }
Write-Output "Pubblicata v$version (SHA-256 $hash)"
