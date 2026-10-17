# Inventaire de Vertuoza et comparaison avec Ubifacture (17/10/2026)

Relevé fait dans le navigateur intégré, sur le compte réel d'Ubique, **en lecture seule** : rien n'a été créé, modifié,
envoyé ni supprimé. Seules manipulations d'affichage : ouverture de menus et de filtres, sélection d'une ligne
(case cochée puis décochée, rien d'enregistré), ouverture puis fermeture sans soumettre de la fenêtre
« Création d'une récurrence », recherche « Location » dans la liste des factures, et passage de la bibliothèque
à 500 lignes par page puis **remise à 20** (préférence d'affichage).
Captures : `docs/vertuoza/captures/inventaire/` (non versionnées : vrais noms de clients). Aucun nom de client ici.
Complète `vertuoza-releve.md` et `vertuoza-formulaires.md` (15/10/2026).

## 1. Ce qu'Ubique utilise vraiment (données présentes)

| Module | Usage constaté | Capture |
|---|---|---|
| Tableau de bord | Devis en cours (8), factures en retard (16), à comptabiliser (1), à envoyer (5), notifications (414 non lues) | 01 |
| Contacts | Particuliers et sociétés ; profils Client, Architecte, Fournisseur – Autres (assurances) | 40 |
| Devis | n° 35 à 57 en 2026 ; statuts Brouillon, À envoyer, Envoyé, Accepté ; filtres ; menu « ⋯ » › Exporter | 02–07 |
| Devis › Totaux | Achats = ventes, marge 0 %, frais généraux 0 % : Ubique **n'utilise pas** les prix d'achat ni la marge | 06 |
| Bibliothèque | 209 ouvrages, aucune catégorie, aucun fournisseur, marge 0 % ; import / export Excel | 44, 45 |
| Chantiers | 20+ chantiers en cours ; tableau de bord (rentabilité, budget d'achat, main d'œuvre), Devis/avenants, Finances (avancements, factures, factures fournisseurs) | 20–22 |
| Factures clients | n° 1 à 124 en 2026 ; acompte, avancement, finale, simple, **récurrente** ; colonne Chantier/Installation | 10, 10b |
| Factures récurrentes | **Location mensuelle de deux lots** (10 factures depuis juin 2026, 21 %) ; menu « Créer/voir la récurrence » | 11–13 |
| Notes de crédit | n° 1 à 5, numérotation à part | 14 |
| Factures fournisseurs | ≈ 600, reçues par Peppol, « À valider » puis « Payé », rattachées à un chantier | 15 |
| Export factures | 4 dossiers PDF (clients/NC et fournisseurs séparés), par trimestre ou deux mois | 16 |
| Statistiques | Général (CA, impayés, conversion, perte, panier moyen) ; Finances (CA mois par mois) | 30, 31 |
| Notifications | « La facture … n°120 a été ouverte » à **chaque** ouverture par le client ; « Une e-facture de … vient d'arriver » | 01 |
| Réglages › E-mails | Modèles **réécrits par Ubique** : Devis, Avenant, Avancement, Facture (par défaut) ; rappels de paiement présents mais **pas activés** | 50 |
| Réglages › Factures | **Facturation annuelle** (numéro qui repart à 1 chaque année fiscale = année civile), VCS affichée, pas de fusion factures / notes de crédit | — |
| Réglages › Espace client | Paiement par QR code activé, couleurs par défaut | — |

Non utilisés (vides) : Opportunités, Fiches techniques, Commandes, Interventions, Installations, Tâches, Plannings
(un ouvrier, rien de planifié), Absences, Conditions particulières (textes sauvegardés), Tâches automatisées.
Preuve de la numérotation annuelle : facture n° 106 du 02/12/2025, puis la numérotation 2026 repart de 1.

## 2. Comparaison avec Ubifacture

| Fonction de Vertuoza utilisée | Ubifacture avant | Maintenant |
|---|---|---|
| Contacts, devis (4 onglets, lots, TVA par ligne, PDF, statuts, dupliquer, historique) | Oui (étapes 4–5) | Inchangé |
| Avenants, avancements, factures d'acompte / avancement / finale, notes de crédit | Oui (étapes 6–7) | Inchangé |
| Paiements, retards, e-mails, rappels, export PDF + UBL | Oui (étape 8) | Inchangé |
| Page client (signer, payer par QR), suivi de chantier, statistiques, tableau de bord | Oui (étape 9) | Inchangé |
| **Factures récurrentes** (location) | **Non** | **Ajouté** : type « Facture récurrente », récurrence depuis une facture, échéances préparées en brouillon d'un clic |
| **Numérotation annuelle** des factures et notes de crédit | Non (compteur jamais remis à zéro) | **Ajouté** : le numéro repart à 1 chaque année ; VCS avec l'année |
| Modèles d'e-mails d'Ubique | Textes écrits par Claude | **Remplacés** par les textes d'Ubique (devis, avenant, facture) |
| [NOM_CHANTIER] dans les e-mails | « n° 3 · PL45 » | **Corrigé** : « PL45 » (la référence du chantier) |
| Notification à chaque ouverture par le client | Seulement la première, dans l'historique | **Ajouté** : bloc « Ouvertures par vos clients » sur le tableau de bord |
| Bibliothèque : import / export Excel (209 ouvrages) | Pas d'import | **Ajouté** : import Excel ou CSV, sans doublons ; fichier des 209 ouvrages relevés prêt à importer |
| Colonne Chantier dans la liste des factures | Non | **Ajouté** |
| Factures fournisseurs (Peppol) | Non | Phase 2 (décision de Mahx) ; en attendant : achats saisis à la main sur le chantier |
| Statut e-facture (Délivré, Paiement transmis) | Non | Phase 2 (Peppol) |
| Exporter en Excel (devis, facture, liste) | « bientôt » | Pas fait : usage non constaté |
| Commentaires (bulle) sur devis / factures | « bientôt » | Pas fait : usage non constaté |
| Acompte défini dans l'onglet Informations du devis | Acompte créé depuis le chantier | Écart accepté à l'étape Design |
| Modèle de mail « Avancement » (état d'avancement envoyé seul) | Non | Pas fait : chez Ubifacture l'avancement part avec sa facture |

## 3. À confirmer par Mahx
- Le numéro affiché sur le PDF des factures de Vertuoza : « 124 » ou « 2026-124 » (le PDF du devis affiche « DEVIS N° 2026-57 ») ? Ubifacture affiche « Facture N° 125 ».
- La communication structurée de Vertuoza (non lisible dans le relevé) : Ubifacture met l'année devant le numéro (2026 + 000125) pour qu'elle reste unique d'une année à l'autre.
- Les deux locations : faut-il créer la récurrence dans Ubifacture dès la mise en service (une facture modèle par lot) ?
