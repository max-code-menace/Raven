# Relevé complet de Vertuoza (lecture seule, 15/10/2026)

Compte relevé : une entreprise du bâtiment belge, pack « Expert » (1/2 comptes gestion, 0/1 compte chantier).
Rien n'a été créé, envoyé ni modifié pendant la visite. Aucun nom de client, montant détaillé ni IBAN n'est repris ici.

Vertuoza mélange deux interfaces : d'anciennes pages PHP (`index.php?mods=X&module=Y`) et de nouvelles pages React (`/web/<société>/...`) intégrées dedans. Les écrans « utilisés » sont ceux qui contiennent des données ; les autres sont vides chez ce client.

## 1. Carte des écrans

| Menu | Écran | Utilisé ? | Notes |
|---|---|---|---|
| Tableau de bord | widgets | oui | Voir §2 |
| Contacts | Entreprises, Contacts | oui | Voir §3 |
| Opportunités | CRM (liste + kanban) | non (vide) | Colonnes : Référence, Commercial, Client, Probabilité, Montant HT, Statut |
| Devis | liste + éditeur | oui | Voir §4 |
| Bibliothèque | 209 lignes de prix, catégories | oui | Voir §5 |
| Fiches Tech. | Fiches, Dossier FT, Groupes | non (vide) | Nom, Marque, Groupe |
| Commandes | Matériaux, Sous-traitant | non (vide) | N°, Date, Chantier, Devis, Type, Fournisseur, Statut |
| Chantiers | liste + fiche chantier | oui | Voir §6 |
| Intervention, Installations, Tâches | listes | non (vide) | |
| Plannings | Chantiers, Gantt (nouveau), Par personne | peu | Planning par personne : 1 ouvrier, vide |
| Finances | Fact. Clients, Notes de crédit, Fact. Fournisseurs, Export Fact. | oui | Voir §7 à §10 |
| Absences | planning des absences | non | Renvoie vers le planning par personne |
| Statistiques | 7 onglets | oui | Voir §11 |
| Notifications | liste (386 non lues) | oui | Factures « ouvertes » par le client, e-factures reçues |
| Réglages | 30 pages | oui | Voir §12 |

## 2. Tableau de bord
Widgets : Mes opportunités, Mes devis (en cours / à envoyer), Mes factures (en retard / à comptabiliser / à envoyer), Mes commandes matériaux, Mes commandes sous-traitants, Mes notes de crédit, notifications (tout / non lu), événements à venir (webinaires), mes tâches, planning par personne. Chaque widget affiche un nombre et un montant, et mène à la liste filtrée.

## 3. Contacts
- **Entreprises** : Nom, Profils, Téléphone, Numéro TVA, E-mail, Adresse.
- **Contacts** (personnes) : Nom, Entreprise, Profils, Téléphone, Téléphone secondaire, E-mail, Adresse.
- Profils possibles : Prospect, Client, Fournisseur – Matériaux, Fournisseur – Sous-traitant, Co-traitant, Architecte, Autre profil, Fournisseur – Autres, Ex-client, Propriétaire.
- Champs d'une fiche : civilité (M., Mme., Inconnu), nom, prénom, entreprise liée, TVA, SIREN, n° chambre de commerce, IDE, identifiant comptable, franco (montant), adresse (+ adresse de facturation distincte), e-mail et e-mail bis, téléphones, site web, conditions de paiement, conditions particulières, IBAN, BIC, taux de TVA par défaut, source, commercial, langue (français, anglais, néerlandais BE, néerlandais), description, métier(s).
- Sources : Apporteur d'affaires, Client, Marketing, Prospection, Recommandation client, Réseaux sociaux, Site web.
- Pays proposés : 16 (Belgique, France, Luxembourg, Pays-Bas, Allemagne, etc.).

## 4. Devis

### Liste
Colonnes : N°, Date, Client, Référence, Montant HT, Responsable, Type de chantier, Statut. Pied de page : Total HT et Total TTC, pagination (20 par page), recherche, filtres.
- **Statuts** : Brouillon, À envoyer, Envoi en cours, Envoyé, Échec envoi, Signé, Accepté (et Refusé).
- **Filtres** : Statut, Date, Client, Client de facturation, Responsable, Gestionnaire, Type de chantier, Commercial, Architecte, Propriétaire.
- **Actions par ligne** : commentaire, modifier, puis menu : Accepter le devis, Refuser, Afficher l'historique, Envoyer par e-mail, Exporter en PDF, Exporter en Excel, Dupliquer, Supprimer.
- Numérotation : un seul compteur continu (ex. devis n° 57), affiché « DEVIS N° 2026-57 ».

### Éditeur : 4 onglets
**Informations**
- Informations générales : Référence*, Responsable*, Date*, Numéro* ; Client* + son adresse.
- Informations de facturation : Taux de TVA par défaut* (autoliquidation 0 %, réduit 6 %, normal 21 %), Conditions de paiement*, interrupteur « coordonnées de facturation identiques au client », Client de facturation*, adresse de facturation.
- Informations du chantier : Architecte, Gestionnaire.
- Remarques : « Remarque 1 (conditions particulières) » et « Remarque 2 ».
- Pièces jointes.

**Encodage** (lignes)
- Structure : **lots** (LOT 1, LOT 2…) qui contiennent des **lignes**, avec glisser-déposer, remarque libre par ligne.
- Colonnes d'une ligne : case, poignée, N°, Type, Description*, Quantité (5 décimales), Unité, Prix unitaire, TVA* (par ligne), Total ; menu « … », corbeille, ajout.
- Types de lignes (bouton Ajouter) : Titre, Sous-titre, Texte, Poste libre, Ouvrage, Fourniture, Équipements, Main d'œuvre, Sous-traitant, Saut de page.
- Réglages d'affichage (engrenage) : numérotation automatique, contrôles de verrouillage, mise en forme, création automatique de lignes, type de ligne par défaut (Poste libre), afficher composants / numéros / type de marché / TVA à la ligne / taux de marge / prix d'achat / prix d'achat total.
- Pied : Marge bénéficiaire (% et €), Total HT. Recherche dans les lignes.

**Totaux**
- Tableau TVA : taux, base HT, TVA.
- Sous-total, « Ajouter un ajustement » (remise, etc.), Total TTC à payer.
- Tableau par type de poste (Postes libres/Ouvrages, Fournitures, Équipements, Sous-traitants, Main d'œuvre) : Achats HT, Frais généraux (% et €), Marge bénéficiaire (% et €), Ventes HT, Temps (heures).

**Visualisation**
- Aperçu PDF page par page. Choix d'un modèle de mise en page (l'entreprise en a un propre) ; le même modèle sert à la signature électronique.
- Disposition du PDF : logo et coordonnées de l'entreprise à gauche, bloc client en haut à droite, titre « DEVIS N° AAAA-N », référence, date, responsable, adresse du chantier, tableau des lots (N°, Description, Qté, Unité, P.U., TVA, Total), tableau TVA, total TTC à payer, conditions de paiement, remarques, pied de page légal.

### Workflow d'un devis (fenêtres cachées trouvées dans la page)
- **Accepter** : date d'acceptation*, type de devis, chantier (création ou rattachement), intervention à planifier, gestionnaire*, affichage sur le planning, dates prévues du chantier. L'acceptation **crée le chantier**.
- **Refuser** : date de refus.
- **Envoyer** : par e-mail avec modèle, lien vers une page en ligne (voir §12 Espace client), signature électronique (statut « Signé »). Envoi payant à l'unité : forfaits de 5 à 100 envois (de 2,20 € à 1,70 € l'envoi).
- **Facturer depuis le devis** : type Facture d'acompte ou Facture finale ; montant en %, € HTVA ou € TTC ; affichage du montant du devis, des avenants, du total à facturer, du déjà facturé et du reste ; « Facturer une tranche » / « Facturer le solde ».
- **Avenants** : devis complémentaire rattaché au chantier.
- Historique des actions, commentaires typés (Notes, E-mail, Appel, RDV), export Excel avec choix du type d'export.
- Un assistant IA (« Vertuo AI ») est proposé dans l'éditeur de devis.

## 5. Bibliothèque
209 lignes. Colonnes : Description, Type, Catégorie, Fournisseur, Unité, Prix d'achat, Marge (puis prix de vente). Panneau Catégories (dont « Sans catégorie »). Sert à insérer des lignes dans les devis. Intégrations de catalogues externes possibles (Batiprix, Cebeo).

## 6. Chantiers
- **Liste** : Numéro, Début de chantier, Référence, Type de chantier, Adresse, Client, Statut (filtre « En cours » par défaut) ; vue liste ou vignettes ; pied : Bénéfice, Reste à facturer, Reste à avancer, Reste à recevoir.
- **Fiche chantier**, 4 onglets :
  - *Tableau de bord* : infos (référence, numéro, client, adresse, dates, gestionnaire, architecte, lien externe), **Rentabilité** (ventes, frais généraux, achats, bénéfice, en %), Bénéfice actuel / prévu, Budget achat (dépensé / disponible), Main d'œuvre (heures planifiées / pointées / disponibles), Facturation (total des factures TVAC, reste à facturer).
  - *Devis/avenants* : détails du devis, récapitulatif (devis de base + avenants).
  - *Finances* : trois sections repliables : **Avancements**, **Factures et notes de crédit**, **Factures fournisseurs**.
  - *Gantt* (nouveau).
- **Avancements** (facturation au pourcentage d'avancement) : N°, Date, Référence, Montant HT, Montant HT révisé, % avancé, % avancé cumulé, Reste, Statut (ex. Facturé). Un avancement génère une « facture d'avancement ».

## 7. Factures clients
- Colonnes : case à cocher, N°, Date, Type, Client, Référence, Statut Fact. Électronique, Statut. Pied : Total HT, Total TTC, Total TVA, Montant à recevoir TTC.
- **Types** : Facture d'acompte, Facture d'avancement (« état d'avancement »), Facture finale, Facture simple, Facture récurrente.
- **Statuts** : Brouillon, À comptabiliser, À envoyer, Envoi en cours, Envoyé, Échec envoi, Partiellement payé, Payé ; plus un filtre « En retard ».
- **Statut facture électronique (Peppol)** : À envoyer, Envoi en cours, Envoyé, Délivré, Accepté, Refusé, Erreur ; « Paiement transmis » apparaît aussi.
- **Filtres** : Statut, Statut Fact. Élec., En retard, Date, Échéance, Date de paiement, Client, Propriétaire, Chantier, Type, Responsable, Type de chantier, Type d'intervention.
- **Actions par ligne** : commentaire, modifier, puis menu : Ajouter le paiement, Afficher l'historique, Envoyer par mail / électroniquement, Exporter en PDF, Exporter en Excel, Supprimer. **Actions groupées** sur sélection.
- **Garde-fou** : modifier une facture comptabilisée ouvre un avertissement (implications juridiques et comptables), propose plutôt une facture corrective ou une note de crédit, et demande de cocher « je comprends les risques ».
- Numérotation : un seul compteur continu (n° 124 à ce jour), partagé selon réglage avec les notes de crédit.

## 8. Notes de crédit
Colonnes : N°, Date, Client, Référence (« Note de crédit pour la facture n°X »), Chantier, Type de chantier, Statut Fact. Élec., Statut. Compteur propre (1, 2, 3…) sauf si la fusion des numéros est activée.

## 9. Factures fournisseurs
- Bandeau « Facturation électronique : recevez vos factures d'achats dans Vertuoza » (Peppol). Des notifications « Une e-facture de X vient d'arriver » préviennent à chaque réception.
- Colonnes : N°, Date, Type (Matériaux, Sous-traitants, Autres factures), Fournisseur, Référence, Montant HT, Chantier, Bon de commande, Statut.
- Paiement : informations de paiement, date, montants ; « Paiement partiel » / « Paiement complet ». Les factures peuvent être rattachées à un chantier et à un bon de commande.

## 10. Export de factures (pour le comptable)
- Liste des exports : N°, Date, Responsable, Période, Format, Contenu, Nb. factures, Statut (Terminé), télécharger / supprimer.
- Formulaire « Création d'un dossier d'export » : **période** (basée sur la date de comptabilisation) ; **types** : factures clients et notes de crédit comptabilisées, factures fournisseurs validées ; **format** : PDF ou **UBL 2.1** ; **contenu** : Toutes ou Uniquement non-exportées.

## 11. Statistiques
Onglets : Général, Devis, Finances, CRM, Commande(s), Intervention(s), Reste à produire. Le Général affiche : chiffre d'affaires, factures clients à payer / en retard (montants et nombres), devis en cours / acceptés, taux de conversion des devis, taux de perte, panier moyen ; filtre par période.

## 12. Réglages (30 pages)
- **Société** : infos générales (logo, nom, nom abrégé, adresse du siège, e-mail, téléphone, site, TVA, banque IBAN/BIC, devise €, **conditions générales en PDF jointes automatiquement aux devis et factures**), horaires, abonnement, adresses de livraison. Les infos légales sont verrouillées tant que Peppol est connecté.
- **Personnel** : employés, ouvriers, indépendants. **Utilisateurs** : comptes gestion, comptes chantier, **rôles** (Administrateur…). Préférences d'affichage.
- **Espace client** : page en ligne où le client consulte, **signe** et **paie** (QR code) devis et factures ; couleur de marque et de fond personnalisables.
- **Modèles d'e-mails** (variables entre crochets : [NOM_CLIENT], [NUMBER_QUOTE], [NO_FACTURE], [NOM_CHANTIER], [REFERENCE], [LINK_DEVIS], [NOM_GESTIONNAIRE], [MAIL_GESTIONNAIRE], [CIVILITE_CLIENT], [DATE_FACTURE]) : Devis, Facture, Avancement, Avenant, versions sous-traitants, Commande matériaux (avec enlèvement / livraison), Commande sous-traitant, Demandes de prix, **deux rappels de paiement** (version douce « Oubli de paiement ? » et version ferme « Rappel de paiement », non activés par défaut).
- **Modèles de documents** : devis, avenants, factures (3 modèles « 2 colonnes » FR/EN/NL + anciens modèles : avec/sans prix, avec/sans détails d'avancement, postes avancés), style général, conditions de bas de page, préférences.
- **Général** : unités (Heure, Jour, Forfait, m², m³, kg, l, boîte… + personnalisées), **taux de TVA** (0 % autoliquidation, 5,5 %, 6 %, 17 %, 21 %, libellés personnalisables, actifs/inactifs, vente/achat), **méthodes de paiement** (Bancontact, Carte de crédit, Chèque, Espèce, Virement), **conditions de paiement** (3, 7, 10, 15, 30, 60 jours après facturation ; 10, 15, 30 jours fin de mois ; comptant), pays, véhicules.
- **Contacts** : civilités, métiers. **Devis** : conditions particulières (textes préenregistrés, liables à un contact), **primes énergétiques** (mentions légales, non utilisé).
- **Chantiers** : préférences, types de chantier, tâches de chantier. **Intervention / Planning / Pointage** : préférences, compétences, types d'installation et d'intervention, frais généraux, types d'absence.
- **Factures (finance)**, la page la plus importante pour nous :
  - Type de facturation : **annuelle** (numérotation repart à 1 chaque année fiscale) ou **mensuelle** (repart à 1 chaque mois, facturation du mois précédent possible jusqu'au 20).
  - **Fusion des numéros** de factures et notes de crédit (numérotation continue commune).
  - **Afficher le numéro VCS** (communication structurée belge pour la réconciliation).
  - **Autoriser la suppression de factures comptabilisées** (déconseillé).
  - Types de frais, **années fiscales** (un exercice doit être ouvert pour comptabiliser), **comptes généraux** (plan comptable, liables aux lignes).
- **Avancé** : tâches automatisées (déclencheurs : envoi du devis, transformation du devis en chantier), **Intégrations**, **Sync. paiement**.
- **Intégrations** : Facturation électronique **Peppol** (connecté), synchro comptable, Testo, Batiprix, Cebeo, **envoi comptable par e-mail** (factures UBL vers le logiciel du comptable).
- **Sync. paiement** : via **CodaBox** (e-mail, numéro de TVA, token à demander) pour rapprocher les paiements des factures.

## 13. Proposition de tri (à valider avec Mahx)

| Fonction | Vertuoza | Proposition |
|---|---|---|
| Clients / fournisseurs / contacts | oui | **Garder** (simplifié : entreprise + contact, TVA, adresse, e-mail, conditions de paiement) |
| Devis avec lots et lignes, TVA par ligne, aperçu PDF | oui | **Garder** (cœur du produit) |
| Statuts devis, accepter / refuser, dupliquer, historique | oui | **Garder** |
| Facture d'acompte / avancement / finale depuis un devis | oui | **Garder** |
| Notes de crédit | oui | **Garder** |
| Numérotation, VCS, années fiscales, verrouillage des factures comptabilisées | oui | **Garder** (obligation légale) |
| Export comptable PDF + UBL 2.1 | oui | **Garder** |
| Envoi par e-mail avec modèles, rappels de paiement | oui | **Garder** |
| Page client en ligne : signature + paiement QR | oui | À trancher (utile, mais gros morceau) |
| Peppol (envoi et réception) | oui | **Phase 2** via un prestataire certifié |
| CodaBox : rapprochement des paiements | oui | **Phase 3** |
| Suivi de chantier : rentabilité, avancements | oui | À trancher (très utilisé : 40 chantiers) |
| Bibliothèque de prix (209 lignes) | oui | **Garder** (import possible) |
| Statistiques | oui | Version réduite (CA, impayés, taux de conversion) |
| Marge, frais généraux, prix d'achat par ligne | oui | À trancher |
| Opportunités / CRM, fiches techniques, commandes, interventions, installations, tâches, planning, absences, pointage | vides chez ce client | **Ne pas copier** |
| Intégrations Testo, Batiprix, Cebeo, assistant IA | non utilisées | **Ne pas copier** |
| Envoi payant à l'unité, abonnement par utilisateur | oui | Sans objet (outil à nous) |

## 14. Décisions de Mahx (15/10/2026)
- **Page client en ligne** (consulter, signer, payer par QR code) : **oui, dès la phase 1**.
- **Suivi de chantier** : **version complète** (rentabilité, budget d'achat, avancements en %, reste à facturer).
- **Marge, frais généraux, prix d'achat à la ligne** : **oui, en option** (interrupteur dans l'éditeur de devis).
- **Types de facture** : **acompte, avancement et finale**, comme dans Vertuoza.
- Toujours à ne pas copier : CRM, fiches techniques, commandes, interventions, installations, tâches, planning, absences, intégrations tierces, assistant IA.
- Toujours en attente : nom du projet, format d'export attendu par le comptable.
