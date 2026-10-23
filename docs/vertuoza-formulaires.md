> **Complément du relevé `vertuoza-releve.md`** (écrit par une autre session le même soir). Ce fichier détaille surtout les **formulaires** (devis, facture finale, avancement, facture fournisseur reçue, avenant, contacts, réglages de facturation) et l'exemple chiffré d'un chantier. En cas de différence sur les statuts ou les filtres, `vertuoza-releve.md` fait foi (il est plus complet). Les captures sont dans `docs/vertuoza/captures/` (non versionnées : vrais noms de clients). Dans ce fichier, « Ubique » désigne l'entreprise du frère de Mahx.
# Ce qu'on a observé dans Vertuoza (15/10/2026)

Source : consultation **en lecture seule** du compte de l'entreprise (Ubique Building Technics) dans le navigateur intégré de Claude, le 15/10/2026. Rien n'a été créé, modifié, envoyé ni enregistré.
Les captures sont dans `docs/vertuoza/captures/` (vrais noms de clients : dossier non versionné, voir `.gitignore`).

Ce fichier ne contient que ce qui a été **vu**. Ce qui est marqué « non vu » n'a pas été regardé : ne pas l'inventer, le regarder dans Vertuoza ou demander à Mahx.
Aucun nom de client n'est repris ici.

## Comment lire Vertuoza avec les outils (astuce technique)
Les écrans sont des pages intégrées (iframe). Pour lire le texte d'un écran, ouvrir directement l'adresse de la page intégrée :
- Devis : `/web/ubique-building-technics/quotes` ; un devis : `/web/ubique-building-technics/quotes/{id}?tab=update`
- Factures clients : `/web/ubique-building-technics/invoices/client-invoices`
- Factures fournisseurs : `/web/ubique-building-technics/invoices/supplier-invoices`
- Réglages : `/include/settings_redirect.php`, puis lire le texte du cadre (`iframe`) de chaque page.
Ne jamais cliquer sur Soumettre, Créer, Envoyer, Supprimer, Accepter, ni cocher « Je comprends les risques ». Le crayon d'une facture envoyée ouvre un avertissement (voir plus bas) : annuler.
**Ne jamais cliquer sur « Nouveau »** dans Vertuoza : dès l'ouverture du formulaire, il crée un **brouillon vide** (constaté le 15/10/2026 sur « Avancements » du chantier PL45 : un avancement « Brouillon » à 0,00 € daté du 15/10/2026 est apparu ; supprimé ensuite avec l'accord de Mahx, le chantier est revenu à son état d'origine : un seul avancement facturé, factures 117 et 122 inchangées). Pour voir un formulaire, ouvrir un élément existant (crayon) et le quitter sans soumettre.

## Menu principal
Tableau de bord · Contacts · Opportunités · Devis · Bibliothèque · Fiches Tech. · Commandes · Chantiers · Intervention · Installations · Tâches · Plannings (Planning Chantiers, Planning Gantt, Planning par personne) · Finances (Fact. Clients, Notes de crédit, Fact. Fournisseurs, Export Fact.) · Absences · Statistiques.
Recherche « Actions » (⌘K), notifications (plus de 99), assistant IA, aide, réglages. Un seul utilisateur vu : le compte partagé.

## Tableau de bord (`tableau-de-bord.png`)
Tuiles : Mes opportunités, Mes devis (en cours, à envoyer), Mes factures (en retard, à comptabiliser, à envoyer), Mes commandes matériaux, Mes commandes sous-traitants, Mes notes de crédit, Mes notifications, Événements à venir.
Notifications vues : « Une e-facture de [fournisseur] vient d'arriver » (reçue par Peppol) ; « La facture … a été ouverte » (**suivi d'ouverture** par le client).

## Devis
### Liste (`devis-liste.jpg`, `devis-menu-actions.jpg`)
Colonnes : **N°**, **Date**, **Client** (icône personne ou entreprise), **Référence** (texte libre, ex. « C44 Volet », « BAH220C Dégâts des eaux (Assurance) »), **Statut** avec date. Recherche, filtres, pagination (20 par page), bouton **Nouveau**, menu « ⋯ » général (non regardé).
Statuts vus : **Brouillon** (gris), **À envoyer** (jaune), **Envoyé** (bleu), **Accepté** (vert). Action « Refuser » existe.
Menu « ⋯ » d'un devis : Accepter le devis · Refuser · Afficher l'historique · Envoyer par email · Exporter en PDF · Exporter en Excel · Dupliquer · Supprimer. Une bulle (commentaires) et un crayon (modifier) par ligne.
Pied de liste : Total HT et Total TTC de la liste.
Les références reprennent souvent un code de chantier (« L63 », « PR33 », « C44 »).

### Formulaire d'un devis (`devis-informations.jpg`, `devis-encodage.jpg`, `devis-visualisation-pdf.jpg`)
Titre « Modifier un devis - {référence} », pastille de statut, **4 onglets : Informations · Encodage · Totaux · Visualisation**.

**Informations**
- *Détails du devis* : Référence*, Responsable* (membre), Date*, Numéro* (modifiable : 57…). Le PDF affiche « DEVIS N° 2026-57 » (année-numéro).
- *Coordonnées du client* : Client* (recherche dans les contacts), adresse affichée.
- *Informations de facturation* : **Taux de TVA par défaut*** (ex. « 6 % (Taux réduit) »), **Conditions de paiement*** (ex. « 7 jour(s) après date de facturation »), **Acomptes** (« Ajouter un acompte »), « Coordonnées de facturation identiques à celles du client » (interrupteur) sinon un autre client de facturation (ex. assurance, syndic).
- *Informations du chantier* : Architecte, Gestionnaire (contacts optionnels), Adresse du chantier (identique à celle du client, interrupteur).
- *Remarques* : Remarque 1 (Conditions particulières) et Remarque 2, avec **textes sauvegardés** réutilisables.
- *Pièces jointes* : import de fichiers, 14 Mo maximum par fichier.

**Encodage** (les lignes)
- Colonnes : case, **N°**, **Type**, **Description*** (texte multiligne), **Quantité** (5 décimales), **Unité**, **Prix U.** (5 décimales), **TVA*** (par ligne), **Total**. Une remarque optionnelle sous chaque ligne. Réglage des colonnes affichées.
- **Types de lignes** : *Titre* (ou lot, « LOT 1 » avec sous-total), *Poste libre* (ligne simple). Le bouton **Ajouter** a un menu avec d'autres types (non regardé : ouvrage, fourniture, sous-total, texte…).
- Chaque ligne a un type de poste (Postes libres, Fournitures, Équipements, Sous-traitants, Main d'œuvre) qui sert à la rentabilité.
- Bas de l'écran : **Marge bénéficiaire** (pourcentage et montant), **Total HT**, « Rentabilité ».
- Les lignes se déplacent par glisser-déposer.

**Totaux**
- Sous-total HT, **Ajouter un ajustement** (remise ou majoration), TVA, **Total TTC à payer**.
- Tableau TVA par taux : base HT et montant de TVA (ex. « TVA 6 % »).
- **Tableau de rentabilité par type de poste** : Achats HT, Frais généraux (% et €), Marge bénéficiaire (% et €), Ventes HT, Temps (heures).

**Visualisation**
- Aperçu PDF du devis, choix d'un **modèle** de document (« Ubique »), bouton Modifier. PDF de 2 pages : en-tête avec logo, bloc client, tableau des lignes, totaux, conditions de paiement, remarques, pied de page, puis page de **signature / bon pour accord**.

## Factures clients (`factures-clients-liste.jpg`, `facture-menu-actions.jpg`)
Colonnes : case, **N°** (continu : 105 à 124), **Date**, **Type**, **Client**, **Référence**, **Statut Fact. Élect.**, **Statut**.
**Types de factures vus** :
- Facture d'**acompte** (« Facture d'acompte #1 »)
- Facture d'**avancement** (« état d'avancement », « Avancement 5# ») : facturation au fur et à mesure du chantier
- Facture **finale**
- Facture **simple** (sans devis)
- Facture **récurrente** (ex. « Location - Lot 1 » : location mensuelle d'un bien)
**Statut** : À envoyer · Envoyé (avec date) · Payé (avec date).
**Statut Fact. Élect.** : À envoyer · Envoyé · **Délivré** (violet) · **Paiement transmis** (« Paim. transmis »).
Pied de liste : Total HT, Total TTC, Total TVA, **Montant à recevoir TTC**.
Cases à cocher et menu **Actions groupées**.
Menu « ⋯ » d'une facture : **Ajouter le paiement** · **Créditer** (note de crédit) · Afficher l'historique · **Envoyer par mail / électroniquement** · Exporter en PDF · Exporter en Excel · Supprimer (grisé une fois envoyée).
**Avertissement vu** en cliquant sur le crayon d'une facture envoyée : « Édition d'une facture comptabilisée. Vous modifiez une facture déjà comptabilisée. Cela peut avoir des implications juridiques et comptables. Alternative : utilisez plutôt une facture corrective ou une note de crédit », avec case « Je comprends les risques et accepte la responsabilité ». → Notre règle : une facture envoyée n'est **jamais** modifiée, seulement créditée.
Formulaire de facture (vu brièvement, brouillon vide) : mêmes onglets que le devis (Informations, Encodage, Totaux, Visualisation). Bloc **Facturation électronique** avec « Référence du bon de commande (fourni par l'acheteur) » et « Engagement juridique » (champs Peppol utiles quand le client est une administration). Statut « Brouillon » existe.
Le détail d'une facture finale et le mécanisme des avancements sont décrits plus bas (section « Chantiers, avenants et avancements »).
**Non vu** : la création d'une facture depuis un devis (étapes exactes), le formulaire d'une facture d'avancement, la déduction éventuelle des acomptes sur la facture finale.

## Factures fournisseurs
Liste : **N°, Date, Type** (ici « Matériaux »), **Fournisseur, Référence** (numéro de la facture du fournisseur), **Chantier** (rattachement à un chantier ou à un devis), **Bon de commande**, **Statut**.
Bannière « **Facturation électronique** : Recevez vos factures d'achats dans Vertuoza ».
Statuts : **À valider** avec une pastille **Peppol** (arrivée par le réseau, en attente de validation par un humain) · **Payé** (avec date).
Volume : environ 30 pages de 20 factures (environ 600) : c'est la grosse part de l'usage quotidien.
Totaux en pied : Total HT, TTC, TVA, Total à recevoir, **Total à payer**.
**Non vu** : l'écran de validation d'une facture reçue (affectation au chantier, TVA, paiement).

## Export de factures (`export-factures.jpg`)
Liste d'exports numérotés avec : N°, date, responsable, **période** (ex. 01/05 – 30/06, 30/06 – 30/09), **Format : PDF**, statut « Terminé », télécharger, supprimer. Bouton **Nouveau**. C'est un lot de PDF des factures d'une période, probablement pour le comptable.
Le comptable peut aussi recevoir ses factures **au format UBL par e-mail** (intégration « Envoi comptable par e-mail ») : à vérifier avec lui.

## Réglages
Rubriques : Société (infos générales, horaire, abonnement, adresses de livraison) · Personnel (employés, ouvriers, indépendants) · Utilisateurs (comptes gestion, comptes chantier, rôles) · Préférences d'affichage · Espace client · Modèles (e-mails, formulaires, documents PDF : style général, conditions de bas de page, modèles devis, avenants, factures) · Propriétés (unités, taux de TVA, méthodes et conditions de paiement, pays, véhicules, civilités, métiers, conditions particulières, primes énergétiques) · Opportunités · Chantiers · Intervention · Planning · Pointage · **Factures** (préférences, types de frais, **fiscalité / exercice**, **comptes généraux**) · Avancé (tâches automatisées, intégration, synchro paiement).

**Taux de TVA** (réglage) : liste de taux avec libellé, type (système ou personnalisé), usage (vente, achat), actif ou non. **Actifs chez Ubique : 21 % « Taux normal », 6 % « Taux réduit », 0 % « TVA acquittée par autoliquidation »** (le libellé est la mention légale affichée sur la facture). Taux inactifs : 5,5 % (France), 17 % (Luxembourg).
**Unités** : Boîte(s), Forfait, Heure(s), Jour(s), kg, l, m², m³, pce (pièce), plus des unités libres ajoutées à la main (« fft », « gl », « H », et même des phrases comme « compris en 04 »). → Dans notre outil : une liste d'unités propre, sans les phrases libres.
**Conditions de paiement** : « X jours après date de facturation » (3, 7, 10, 15, 30, 60), « Fin du mois » (10, 15, 30), « Paiement comptant » (0).
**Méthodes de paiement** : Bancaire, Bancontact, Carte de crédit, Chèque, Espèce, Virement.
**Intégrations** : *Facturation électronique (Peppol) : connectée* · Synchro comptable (non connectée) · Envoi comptable par e-mail (UBL) · Batiprix et Cebeo (catalogues de prix, non connectés) · Testo.
**Non vu** : infos de la société (logo, numéro de TVA, IBAN), préférences de facturation (numérotation, mentions), comptes généraux, sync paiement (rapprochement bancaire ?), modèle PDF des factures.

## Chantiers, avenants et avancements (le cœur de la facturation)
Un **chantier** naît d'un devis accepté. Liste : **Numéro** (ex. 574354), début, **Référence** (= référence du devis), type de chantier, adresse, statut (« En cours »). Pied de liste : Bénéfice, Reste à facturer total, Reste à avancer total, Reste à recevoir total.
Fiche d'un chantier : 4 onglets **Tableau de bord · Devis/avenants · Finances · Gantt**.
- *Tableau de bord* : infos (référence, numéro, client, adresse, dates, gestionnaire, architecte), **rentabilité** (bénéfice actuel et prévu, total des ventes, frais généraux, total des achats), budget d'achat dépensé et disponible, main-d'œuvre (heures planifiées et pointées), facturation (total des factures TVAC, reste à facturer TVAC).
- *Devis/avenants* : numéro et référence du devis, **date d'acceptation**, récapitulatif : **Devis de base**, **Avenants**, **Total devis/avenants**. Exemple vu : devis de base 10 384,13 € + un avenant de −3 780,36 € = 6 603,77 € HT. Un **avenant** corrige le devis (en plus ou en moins) après acceptation.
- *Finances* : trois sections.
  1. **Avancements** : colonnes N°, Date, Référence, **Montant HT**, **% avancé**, **% avancé cumulé**, **Reste à avancer HT**, Statut (« Facturé »). Bouton Nouveau.
  2. **Factures et notes de crédit** du chantier (type, statut, statut e-facture) avec totaux HT / TTC / TVA / montant à recevoir.
  3. **Factures fournisseurs** rattachées au chantier (non dépliée).

**Mécanisme observé sur un chantier** : total devis + avenants = 6 603,77 € HT (7 000,00 € TTC). Un avancement de **55,04 %** crée une **facture d'avancement** de 3 634,47 € HT ; la **facture finale** reprend le **reste à avancer** : 2 969,30 € HT (TVA 6 % : 178,16 €, total TTC 3 147,46 €). Total facturé = total du devis.
La **facture d'acompte** est définie dans l'onglet Informations du devis (« Acomptes »), pas dans les avancements.

## Facture finale (vue d'une facture « À envoyer », sans enregistrer)
Mêmes onglets que le devis. Bloc **« Lié à un chantier/installation »** : Chantier (numéro, référence, client) et Installation. Taux de TVA par défaut, conditions de paiement, coordonnées de facturation, bloc Facturation électronique, remarque, pièces jointes.
**Encodage** : lignes **reprises du devis** avec numérotation hiérarchique (1.0 titre, 1.1 texte, 2.0 titre, 2.1, 2.2…), types **Titre**, **Texte** (description libre), lignes de poste. Colonnes : N°, Type, Description, Quantité, Unité, Prix U., TVA, Total, actions par ligne (menu, supprimer, ajouter). Total HT en bas.
**Menu « Ajouter »** (valable pour devis et factures) : **Titre · Sous-titre · Texte · Poste libre · Ouvrage · Fourniture · Équipements · Main d'œuvre · Sous-traitant**.
**Totaux** : sous-total HT, TVA, total TTC à payer, tableau TVA par taux. Pas de ligne « acomptes déjà payés » vue sur cette facture (non vu dans le tableau, à confirmer).

## Notes de crédit
Numérotation **à part** (1 à 5 seulement : rares). Colonnes : N°, Date, Client, **Référence** (« Note de crédit pour la facture n°94 »), **Chantier**, Statut Fact. Élect., Statut. Pied : Total HT, TTC, TVA. Créée depuis le menu « Créditer » d'une facture.
**Non vu** : le formulaire d'une note de crédit (crédit total ou partiel ?).

## Bibliothèque de prix
209 éléments, arborescence de catégories (aucune utilisée : tout est « Sans catégorie »), recherche, filtres, pagination.
Colonnes : Description, Type (icône), Catégorie, Fournisseur, Unité. Exemples : « Prise double bipolaire + terre + sécurité » (pce), « Tableau divisionnaire complet » (fft), « Réception par organisme agréé » (fft).
**Fiche d'un « ouvrage »** : Description*, Catégorie, Unité*, **Prix d'achat***, **Marge***, **Prix de vente unitaire***, Photo, Remarque, **Composants** (lignes qui composent l'ouvrage) et **Compléments** (lignes en plus).
Un ouvrage inséré dans un devis donne une ligne avec description, unité et prix. Mise à jour et import de catalogues : Batiprix et Cebeo (non connectés).

## Formulaire d'avancement (le plus important à reproduire)
Page « Avancements » ouverte depuis le chantier (Finances → Avancements → Nouveau). Le tableau des avancements d'un chantier a les colonnes : N°, Date, Référence, Montant HT, **Montant HT révisé**, % avancé, % avancé cumulé, Statut (**Brouillon** ou **Facturé**) ; le crayon est grisé une fois facturé.
**Informations générales** : Référence*, Numéro de l'avancement*, Chantier, Client, Contacts (Architecte), Responsable, **Conditions de paiement*** (3, 7, 10, 15, 30, 60 jours après facturation ; comptant ; 10, 15, 30 jours fin de mois), **Date de l'avancement***, « Définir une période » (oui / non).
**Encodage de l'avancement** : on reprend **toutes les lignes du devis** (numérotées 1.0, 1.1, 2.0, 2.1…) avec les colonnes : N°, Art., Description, **Qté**, TM, **Unité**, **P/U (€)**, **Total (€)**, puis trois blocs : **Avancement précédent** (Qté, %, €), **Avancement** de la période (Qté, %, €), **Avancement final** (cumul : Qté, %, €), et une remarque. Chaque ligne s'avance en quantité, en pourcentage ou en euros.
Les **avenants** sont listés à la suite du devis comme un second bloc (« Avenant n°2 », ligne « AVENANT Négatif : travaux non exécutés / ajustement convenu », prix −3 780,36 €), avancés de la même façon.
**Totaux en pied** : Total devis, avancement précédent, avancement de la période, avancement total ; pour chaque avenant pareil ; puis **Total chantier HT** (6 603,77 €), **Total avancement** (3 634,47 €), **Reste à produire HT** (2 969,30 €) ; enfin le sous-total HT, la TVA par taux et le Total TTC à payer de la facture qui en résulte.
Exemple de devis lu : titres de lots (« Fourniture des matériaux », « Main-d'œuvre »), lignes de matériaux à l'unité (pce, ml, m², fft) et lignes de main-d'œuvre par jour (« 16/09 2 hommes 8 heures », 16 h à 50 €).

## Contacts
Liste : Nom, Entreprise, **Profils**, Téléphone, Téléphone secondaire ; sous-menu Entreprises et Contacts. Profils vus : **Client, Prospect, Fournisseur - Autres, Architecte**. Les assurances sont des fournisseurs « Autres » (clients de facturation d'un devis d'assurance).
**Fiche d'un contact (particulier)**, onglets Général et Avancé : Nom*, Prénom, Civilité, Entreprise (lien), Profils*, E-mails (plusieurs), Téléphones (plusieurs), adresse (rue, numéro, code postal, localité, pays), coordonnées de facturation identiques ou non, remarque, **IBAN, BIC**, Source, Commercial, et des **valeurs par défaut** : conditions de paiement, TVA, remarque, franco, frais.
**Fiche d'une entreprise** : Nom de l'entreprise*, **N° TVA**, puis les mêmes champs.

## Commandes fournisseurs
Commandes matériaux : colonnes N°, Date, Chantier, Devis, Type, Statut ; **aucune commande** enregistrée (« Aucune commande matériaux », total 0,00 €). Le tableau de bord montre aussi 0 commande de sous-traitants. → Pas utilisées aujourd'hui : **hors périmètre**. Les achats passent directement par les factures fournisseurs.

## Validation d'une facture fournisseur reçue (Peppol)
Écran « Modifier une facture fournisseur - {numéro du fournisseur} », statut **À valider**. À gauche le formulaire, à droite le **document reçu** (PDF avec zoom, importer, supprimer, télécharger ; le contenu structuré de la e-facture est affiché : vendeur, acheteur, paiement, lignes, taxes).
Champs : **Type de facture*** (ex. Matériaux), **Référence*** (numéro du fournisseur), **Date***, **Fournisseur*** (contact), **Date d'échéance**, **Montant TTC***, **Montant HT***, **Taux TVA*** + **Montant TVA*** avec « Ajouter une TVA » (plusieurs taux), **Coordonnées de paiement** : IBAN, BIC/Swift, **Communication** (structurée ou libre).
En bas : **« Lier la facture à un chantier »** : une ou plusieurs liaisons (chantier + montant HT), avec les trois totaux **Montant HT de la facture / Montant total lié / Montant restant à lier**. Une facture peut donc être répartie sur plusieurs chantiers. Les champs HT, TVA et la date sont pré-remplis depuis la e-facture quand elle vient de Peppol.
Statuts d'une facture fournisseur : À valider (avec pastille Peppol), Payé (avec date). Les factures reçues indiquent le moyen de paiement du fournisseur (virement ou débit direct SEPA) et l'échéance.

**Relevé du 23/10/2026 (Claude in Chrome, lecture seule)** : le filtre « Statut » de la liste propose **Brouillon, À valider, À payer, Part. payé, Payé, Refusé** ; autres filtres : En retard, Date, Date de création, Fournisseur, Source, Type, Chantier, Type de frais. Pied de la liste : Total HT, Total TTC, Total TVA, Total à recevoir (négatif : les notes de crédit), **Total à payer**. Menu « ⋯ » d'une facture payée : « Supprimer le dernier paiement », « Exporter en PDF », « Afficher l'historique », « Supprimer ». Historique de deux factures payées : « Création d'une facture » sans utilisateur (arrivée par Peppol), puis, par un utilisateur et dans la même minute, « Validée » (Accepté) et « Paiement complet : montant » (Payé). Le paiement est donc **noté à la main** au moment de la validation, pas par un lien bancaire (CodaBox) sur ces factures-là.

## Avenants
Section « Avenants » du chantier (onglet Devis/avenants) : colonnes **N°, Date, Référence, J. Ouv.** (jours ouvrables supplémentaires accordés), **Montant HT, Statut** (« Accepté » avec date). Pied : **Total J. Ouv. Supp**. Le numéro d'avenant continue la numérotation du chantier (« Avenant n°2 »).
Un avenant accepté **n'est plus modifiable** (crayon grisé). Menu : Retour au statut précédent · Afficher l'historique · Envoyer par email · Exporter en PDF · Exporter en Excel · Dupliquer. Il suit donc le même cycle que le devis (brouillon, envoyé, accepté). Il existe aussi un avenant « Import avenant » (créé par import, ligne unique au forfait).
**Non vu** : le formulaire de création d'un avenant (ne pas cliquer sur Nouveau).

## Réglages société et factures
**Société** : logo (import), société*, nom abrégé*, adresse (rue, numéro, code postal, ville, pays), e-mails (plusieurs), téléphones (plusieurs), site web, **TVA, numéro d'entreprise et identifiants** (SIREN, KVK, IDE, RCS, capital social, APE selon le pays), **IBAN et BIC avec plusieurs comptes**, **QR-IBAN**, devise (EUR), **conditions générales** (texte joint aux documents), **assurances** (nom de l'assureur, garantie, zone de couverture, pays : mention d'assurance décennale sur les documents).
**Préférences de facturation** (`mods=admin&module=fin_settings`) :
- **Type de facturation** : annuelle (numérotation repart à 1 chaque année fiscale) ou mensuelle (repart à 1 le premier du mois ; facturer sur le mois précédent possible jusqu'au 20).
- **Fusion des numéros de factures** : factures et notes de crédit partagent une numérotation continue (option ; chez Ubique les notes de crédit ont leur propre numérotation).
- **Afficher le numéro VCS** : communication structurée belge sur les factures, pour que le client la mette dans son virement (réconciliation).
- Utilisation du QR-facture (Suisse), du **QR-IBAN**, de l'**arrondi supérieur**.
- **Suppression des factures comptabilisées** : option déconseillée (« complications légales et perturbations comptables »). → Chez nous : jamais.
**Synchronisation des paiements** (`sync_paiement`) : choix du logiciel **CodaBox**, avec e-mail, numéro de TVA et token (demande d'un jeton). → C'est ainsi que Vertuoza rapproche les paiements (phase 3). L'état de la connexion n'a pas été vérifié.
**Comptes généraux** : liste vide, donc pas de plan comptable utilisé dans Vertuoza ; le comptable travaille sur les PDF (export) ou le format UBL par e-mail.
**Fiscalité** (exercice) : page non lisible en texte, non vue.

## Non regardé du tout
Opportunités, Fiches Tech., Interventions, Installations, Tâches, Plannings, Absences, Statistiques, formulaire d'avenant (création), formulaire de note de crédit (Vertuoza demande de cocher « Je comprends les risques » avant d'ouvrir une note déjà envoyée : non fait), modèles PDF (devis, factures, conditions de bas de page), modèles d'e-mails.

## Ce qu'on retient pour Ubique
1. **Flux réel** : devis → envoyé → accepté → **facture d'acompte**, puis **factures d'avancement** pendant le chantier, puis **facture finale**. Les factures d'avancement sont fréquentes : c'est une fonction centrale, pas un détail.
2. **Factures fournisseurs** : très nombreuses (environ 600), arrivées par Peppol, à valider puis à payer, rattachées à un chantier. C'est probablement ce qui coûte le plus à refaire (phase 2).
3. **Location** : des factures récurrentes mensuelles existent (location de lots) : à confirmer si cela fait partie du besoin.
4. **Règle à reprendre** : facture envoyée = intouchable, correction par note de crédit.
5. **Rentabilité** par devis (achats, frais généraux, marge, heures) : utile mais pas pour la phase 1.
6. Le suivi d'ouverture d'une facture par le client et le suivi du statut d'e-facture (délivré, paiement transmis) sont visibles dans le quotidien.
