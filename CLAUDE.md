# Ubifacture — instructions pour Claude Code

## Le produit
Outil de **devis, de factures et de suivi de chantier** pour une entreprise du bâtiment belge (Ubique, l'entreprise du frère de Mahx).
Il remplace Vertuoza (environ 300 € par mois). Un seul compte partagé par deux personnes. Clients : particuliers et entreprises.

Décisions fermes, à ne pas remettre en question :
- **Projet en Rust.** Ne jamais proposer un autre langage ni un framework JavaScript.
- **Projet séparé de Nodimmo** : dossier, base de données et dépôt propres. Ne jamais toucher à `..\nodimmo` ni à `..\ubique` (ancien dossier de documentation, remplacé par celui-ci).
- On ne copie pas tout Vertuoza : seulement ce que l'entreprise utilise (voir « Décisions de Mahx »).
- Outil interne : pas de paiement en ligne de ce logiciel, pas de facturation de ce logiciel.

## Décisions de Mahx (15/10/2026)
- **Page client en ligne** (consulter, signer, payer par QR code) : **oui, dès la phase 1**.
- **Suivi de chantier complet** : rentabilité, budget d'achat, avancements en %, reste à facturer.
- **Marge, frais généraux, prix d'achat à la ligne** : oui, **en option** (interrupteur dans l'éditeur de devis).
- **Types de facture** : acompte, avancement, finale, comme dans Vertuoza (plus la facture simple, la récurrente est à confirmer).
- **À ne pas copier** : CRM, fiches techniques, commandes (vides chez eux), interventions, installations, tâches, planning, absences, intégrations tierces (Testo, Batiprix, Cebeo), assistant IA.
- Toujours en attente de réponse : format d'export attendu par le comptable (PDF par période ou UBL par e-mail), numérotation actuelle à poursuivre (dernière facture n° 124, dernier devis n° 57, 5 notes de crédit), annuelle ou mensuelle.

## Qui code
Mahx dirige le produit, il n'est pas développeur. Claude code.
- Avancer étape par étape, une fonctionnalité à la fois, et attendre son feu vert avant la suivante.
- Donner les commandes exactes à lancer (Windows, PowerShell).
- Expliquer chaque partie en une ou deux phrases simples, en français, en tutoyant.
- Quand il colle une erreur du compilateur, la corriger directement.
- Toujours lancer `cargo build` et `cargo test` avant de dire qu'une étape est finie.
- **Ne rien inventer sur Vertuoza.** Tout ce qu'on en sait est dans `docs/vertuoza-releve.md` (carte complète des écrans, statuts, filtres, réglages) et `docs/vertuoza-formulaires.md` (détail des formulaires : devis, avancement, facture finale, facture fournisseur reçue, avenant, contacts, réglages de facturation). Captures dans `docs/vertuoza/captures/`. Pour un point non documenté : demander à Mahx ou regarder Vertuoza dans le navigateur intégré, **en lecture seule**.
- **Règles de lecture seule sur Vertuoza** : ne jamais cliquer sur **Nouveau** (il crée un brouillon vide dès l'ouverture : c'est arrivé), Soumettre, Envoyer, Supprimer, Accepter, ni cocher « Je comprends les risques ». Pour voir un formulaire, ouvrir un élément existant (crayon) et le quitter sans l'enregistrer. Les écrans sont des pages intégrées : ouvrir l'adresse `/web/<société>/...` pour lire leur texte. Ne reprendre aucun nom de client dans les fichiers versionnés.
- Mettre à jour la section « État du projet » à la fin de chaque étape.

## Stack imposée
- Rust, édition 2024. Axum 0.8 (routes en `Router`, `src/lib.rs` assemble les modules), Askama 0.16 (templates dans `templates/`), htmx (pas de JavaScript écrit à la main), Tailwind CSS + DaisyUI (via CDN pour l'instant ; installation locale avant la mise en ligne).
- PostgreSQL + SQLx 0.9 (migrations numérotées dans `migrations/`, **montants en centimes**, enums Postgres pour les statuts). Requêtes SQLx à l'exécution (pas de `query!`).
- Base `ubifacture`, utilisateur `ubifacture` (créés par `scripts/creer-base.sql`), `DATABASE_URL` dans `.env` (non versionné, modèle dans `.env.example`). Les tests créent leurs propres bases temporaires.
- Lancement : `cargo run` (port 3001, voir `.claude/launch.json`).
- Choix à confirmer avec Mahx au moment de l'étape concernée : génération des PDF, connexion (Argon2 + cookie de session, comme Nodimmo), e-mails, signature en ligne, paiement par QR, hébergement en UE.

Machine de Mahx : Windows. Build Tools Visual Studio installés.

## Vocabulaire (le même partout : code, écrans, PDF)
| Terme | Sens |
|---|---|
| Entreprise / Contact | Fiches de contacts : une société ou une personne. Profils : prospect, client, fournisseur (matériaux, sous-traitant, autres), architecte, etc. |
| Devis | Offre de prix à lots et lignes. Statuts : brouillon, à envoyer, envoi en cours, envoyé, échec d'envoi, signé, accepté, refusé |
| Chantier | Naît d'un devis accepté ; porte avancements, factures et factures fournisseurs |
| Avenant | Correction du devis après acceptation (en plus ou en moins) ; jours ouvrables supplémentaires possibles |
| Facture d'acompte | Émise après acceptation du devis, avant les travaux |
| Avancement / Facture d'avancement | Pourcentage d'avancement du chantier, qui produit une facture ; la facture finale reprend le reste à avancer |
| Facture finale | Solde du chantier |
| Note de crédit | Facture négative qui corrige une facture envoyée (jamais de modification d'une facture envoyée) |
| Communication structurée (VCS) | `+++123/4567/89012+++`, calculée par le serveur (modulo 97) |
| Ouvrage | Élément de la bibliothèque de prix : prix d'achat, marge, prix de vente, composants |
| Types de ligne | Titre (lot), sous-titre, texte, poste libre, ouvrage, fourniture, équipements, main d'œuvre, sous-traitant, saut de page |

## Règles métier belges (à faire valider par le comptable avant la mise en service)
1. TVA : 6 % (rénovation de logements de plus de 10 ans), 21 %, 0 % avec la mention « TVA acquittée par autoliquidation » entre entreprises du bâtiment.
2. Numérotation des factures **continue, sans trou**, jamais réutilisée ; compteur incrémenté dans la même transaction que la création (déjà fait pour les devis). Une facture envoyée ne se modifie plus : on la corrige par une note de crédit. Pas d'option pour supprimer une facture comptabilisée.
3. Dates stockées en `timestamptz` (ou `date` pour une date de document), affichées 15/10/2026 ; montants affichés « 4 431,84 € ».
4. Mentions obligatoires d'une facture belge, numéro de TVA, IBAN, délai de paiement, conditions générales et assurance décennale sur les documents : à valider avec le comptable.
5. Exemple vérifié sur un vrai chantier : devis de base 10 384,13 € HT + avenant −3 780,36 € = 6 603,77 € HT ; avancement de 55,04 % = 3 634,47 € ; facture finale = reste à avancer 2 969,30 € HT.

## Phases
- **Phase 1** : contacts, devis (lots, lignes, TVA, option marge et prix d'achat, aperçu et export PDF), accepter / refuser, chantier, avancements, factures d'acompte / avancement / finale / simple, notes de crédit, numérotation, VCS, suivi des paiements et retards, export pour le comptable (PDF, UBL 2.1 si demandé), envoi par e-mail avec modèles et rappels, **page client en ligne** (consulter, signer, payer par QR), suivi de chantier complet, bibliothèque de prix, statistiques réduites.
- **Phase 2 : Peppol.** Envoi et réception d'e-factures (obligatoire entre entreprises en Belgique depuis le 1er janvier 2026). Il faut un **point d'accès Peppol certifié** (prestataire agréé avec API) : on ne se connecte pas au réseau soi-même. Environ 600 factures fournisseurs à valider et rattacher à un chantier : formulaire pré-rempli depuis la e-facture, liaison à un ou plusieurs chantiers.
- **Phase 3 : CodaBox.** Lecture des extraits bancaires CODA et rapprochement des paiements par la communication structurée.

## État du projet
- [x] Étapes 1 à 4 : squelette Axum + Askama + htmx + DaisyUI ; PostgreSQL + SQLx (migrations `0001_base.sql`, `0002_societe_fichiers.sql`) ; écrans **Entreprises** et **Contacts** (liste, recherche en direct, filtre par profil, création, modification) ; écran **Société** (identité, adresse, contacts multiples, TVA belge, comptes bancaires, logo, conditions générales). Relevé de Vertuoza dans `docs/`.
- [x] Étape 5a : migration `0003_devis.sql` (table `devis`, statuts de Vertuoza, compteur de numéros) ; écran `/devis` avec recherche, filtres statut et dates, tri, 20 par page, totaux HT et TTC ; le bouton Nouveau crée un brouillon. 77 tests au dernier commit.
- [ ] Étape 5b : éditeur de devis (onglets Informations, Encodage, Totaux, Visualisation ; lots et lignes ; TVA par ligne ; totaux calculés par le serveur). Découpée en :
  - [x] 5b-1 : migration `0004_devis_informations.sql`, page `/devis/{id}` avec l'onglet **Informations** (référence, responsable, date, client, TVA par défaut, conditions de paiement, client de facturation, architecte, gestionnaire, adresse du chantier, remarques). Seul un brouillon se modifie ; « Nouveau » ouvre directement l'éditeur ; le numéro n'est pas modifiable (numérotation continue). 93 tests. Pièces jointes et acomptes : pas encore (acomptes à l'étape 7).
  - [x] 5b-2 : migration `0005_devis_lignes.sql`, `src/calcul.rs` (totaux en entiers : quantités et prix en 1/100 000, arrondi au centime, TVA calculée par taux sur la somme des bases), `src/devis_lignes.rs`, onglet **Encodage** `/devis/{id}/encodage` : titres (lots 1.0, 2.0 avec sous-total), sous-titres, textes, postes libres ; chaque champ s'enregistre à la sortie du champ (htmx), les totaux HT / TVA par taux / TTC se recalculent et sont enregistrés dans le devis ; monter / descendre (boutons, pas de glisser-déposer pour éviter le JavaScript) et supprimer. 118 tests. Pas encore : remarque par ligne, autres types de ligne.
  - [x] 5b-3 : migration `0006_devis_ajustement.sql`. Onglet **Totaux** `/devis/{id}/totaux` : sous-total, **ajustement** (une remise ou majoration, en % ou en montant HT, avec libellé), tableau de TVA par taux, total TTC. Règle choisie (à faire valider par le comptable) : l'ajustement porte sur le total HT et se répartit entre les taux de TVA au prorata des bases (sans perdre un centime), la TVA se calcule ensuite par taux. Onglet **Visualisation** `/devis/{id}/apercu` : aperçu HTML du devis (société, logo, client, « facturé à », adresse du chantier, lots et lignes, totaux, conditions de paiement, remarques, IBAN) ; le PDF reprendra cette mise en page à l'étape 5c. 142 tests. Pas encore : tableau par type de poste (achats, frais généraux, marge).
  - [x] 5b-4 : migrations `0007_genres_ligne.sql` et `0008_marge_prix_achat.sql`. Types de ligne ajoutés : ouvrage (sans bibliothèque pour l'instant), fourniture, équipements, main d'œuvre (unité « h » par défaut), sous-traitant, saut de page (visible seulement dans l'aperçu). **Remarque par ligne.** **Option « prix d'achat, marge et frais généraux »** (interrupteur dans l'Encodage, enregistré dans le devis ; éteinte, les prix d'achat sont gardés) : colonnes Achat U. et Marge par ligne, frais généraux en % des achats, marge totale ; le tableau **Rentabilité** par type de poste est dans l'onglet Totaux (achats, frais généraux, marge, taux, ventes, heures ; l'ajustement réduit les ventes et la marge du total). Définitions choisies (à faire valider par Mahx) : marge = ventes − achats − frais généraux ; taux de marge = marge ÷ (achats + frais généraux) ; les heures ne comptent que pour la main d'œuvre en « h / heure(s) ». Le devis du client n'affiche jamais les prix d'achat ni la marge. 165 tests. Étape 5b terminée.
- [ ] Étape 5c : aperçu et export PDF du devis, accepter / refuser, dupliquer, historique.
- [ ] Étape 6 : chantier (créé à l'acceptation), avenants, avancements.
- [ ] Étape 7 : factures (acompte, avancement, finale, simple), notes de crédit, numérotation, VCS.
- [ ] Étape 8 : paiements et retards, export pour le comptable, e-mails.
- [ ] Étape 9 : page client en ligne, suivi de chantier complet, bibliothèque de prix, statistiques.
- [ ] Phases 2 et 3.

## Fichiers de documentation
- `docs/vertuoza-releve.md` : relevé complet de Vertuoza et décisions de Mahx.
- `docs/vertuoza-formulaires.md` : détail des formulaires et exemple chiffré.
- `docs/plan-vertuoza-rust.html` : première version du plan, **dépassée** par ce fichier.
