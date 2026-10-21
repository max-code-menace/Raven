# Essai de la reception Peppol (etape 5-3), dans le BAC A SABLE de Recommand uniquement.
# Fait envoyer une facture d'essai par "Vendeur Test SRL" a "Essai Peppol" (organisation n. 2 de Raven chez Mahx).
# Ensuite, dans Raven : Finances > Factures fournisseurs > "Relever".
#
# Lancement (PowerShell normal, dans le dossier du projet) :
#   powershell -ExecutionPolicy Bypass -File scripts\essai-reception-peppol.ps1
# Avec une note de credit au lieu d'une facture :
#   powershell -ExecutionPolicy Bypass -File scripts\essai-reception-peppol.ps1 -NoteDeCredit
#
# Les cles sont lues dans le fichier .env (RECOMMAND_CLE_API, RECOMMAND_SECRET). Rien ne part sur le vrai reseau
# tant que ce sont les cles du bac a sable.
param(
    [string]$Destinataire = "0208:0100000268",
    [string]$Vendeur = "c_01M5DX6E5SGG8MDN3A2W98W9PZ",
    [switch]$NoteDeCredit
)

$ErrorActionPreference = "Stop"
$reglages = @{}
Get-Content ".env" | ForEach-Object {
    if ($_ -match '^\s*([A-Z_]+)\s*=\s*(.*)\s*$') { $reglages[$Matches[1]] = $Matches[2].Trim('"') }
}
if (-not $reglages["RECOMMAND_CLE_API"] -or -not $reglages["RECOMMAND_SECRET"]) {
    Write-Host "RECOMMAND_CLE_API et RECOMMAND_SECRET manquent dans .env" -ForegroundColor Red
    exit 1
}
$jeton = [Convert]::ToBase64String([Text.Encoding]::ASCII.GetBytes($reglages["RECOMMAND_CLE_API"] + ":" + $reglages["RECOMMAND_SECRET"]))
$aujourdhui = Get-Date -Format "yyyy-MM-dd"
$numero = "ESSAI-" + (Get-Date -Format "yyyyMMdd-HHmmss")

$acheteur = @{ name = "Essai Peppol"; street = "Rue de l'Essai 1"; city = "Wavre"; postalZone = "1300"; country = "BE"; vatNumber = "BE0100000268" }
$lignes = @(
    @{ name = "Blocs beton 14 cm"; description = "Palette de 60 blocs"; quantity = "5.00"; unitCode = "C62"; netPriceAmount = "62.40"; vat = @{ category = "S"; percentage = "21.00" } },
    @{ name = "Ciment 25 kg"; quantity = "12.00"; unitCode = "C62"; netPriceAmount = "8.15"; vat = @{ category = "S"; percentage = "21.00" } }
)
if ($NoteDeCredit) {
    $type = "creditNote"
    $document = @{ creditNoteNumber = $numero; issueDate = $aujourdhui; buyer = $acheteur; lines = $lignes }
} else {
    $type = "invoice"
    $document = @{ invoiceNumber = $numero; issueDate = $aujourdhui; dueDate = (Get-Date).AddDays(30).ToString("yyyy-MM-dd"); buyer = $acheteur
        paymentMeans = @(@{ iban = "BE68539007547034"; reference = "000041200048" }); lines = $lignes }
}
$corps = @{ recipient = $Destinataire; documentType = $type; document = $document } | ConvertTo-Json -Depth 10

try {
    $reponse = Invoke-RestMethod -Method Post -Uri "https://app.recommand.eu/api/v1/$Vendeur/send" `
        -Headers @{ Authorization = "Basic $jeton" } -ContentType "application/json" -Body $corps
    Write-Host "Envoye : $numero (document $($reponse.id), statut $($reponse.deliveryStatus))." -ForegroundColor Green
    Write-Host "Dans Raven (organisation Essai Peppol) : Finances > Factures fournisseurs > Relever."
} catch {
    Write-Host "Refuse par Recommand :" -ForegroundColor Red
    if ($_.ErrorDetails) { Write-Host $_.ErrorDetails.Message } else { Write-Host $_.Exception.Message }
    exit 1
}
