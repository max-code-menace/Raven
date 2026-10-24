# Refabrique assets/web/raven.css : les classes de Tailwind employées dans templates/ et src/ (étape 7-2).
# À lancer après avoir ajouté ou changé des classes de Tailwind dans un gabarit (les tests de GitHub le vérifient).
# L'outil (un seul fichier, sans Node) est téléchargé une fois dans outils/ (hors du dépôt), à la version 3.4.17, celle
# qu'employait le CDN : le CSS fabriqué est le même que celui que le navigateur fabriquait avant.
#   powershell -ExecutionPolicy Bypass -File scripts\css.ps1
$ErrorActionPreference = "Stop"
$racine = Split-Path -Parent $PSScriptRoot
$outil = Join-Path $racine "outils\tailwindcss.exe"
$empreinte = "67f1c5e3f5a03406a7bf5badf5ada09b79f3ae78ec43450c15f7e983068da346"
if (-not (Test-Path $outil)) {
    New-Item -ItemType Directory -Force (Join-Path $racine "outils") | Out-Null
    Write-Host "Téléchargement de Tailwind 3.4.17 (40 Mo, GitHub)…"
    Invoke-WebRequest -Uri "https://github.com/tailwindlabs/tailwindcss/releases/download/v3.4.17/tailwindcss-windows-x64.exe" -OutFile $outil
}
if ((Get-FileHash $outil -Algorithm SHA256).Hash.ToLower() -ne $empreinte) {
    Remove-Item $outil
    throw "L'outil Tailwind téléchargé n'a pas l'empreinte attendue : il a été supprimé."
}
Push-Location $racine
try {
    & $outil -o assets/web/raven.css --content "./templates/**/*.html,./src/**/*.rs" --minify
    if ($LASTEXITCODE -ne 0) { throw "Tailwind a échoué." }
    Write-Host "assets/web/raven.css refabriqué. Relancer cargo build pour qu'il entre dans le programme."
} finally {
    Pop-Location
}
