# Conformité belge : ce qui est fait, ce qui reste à faire valider

Document de travail tenu à jour avec le code. Date de la recherche : **19/10/2026**.

## D'où viennent ces règles

Trois recherches faites avec Claude (conformité TVA et facturation ; analyse juridique du lancement d'un SaaS ; contrôle de ces
réponses), plus une recherche produit et réglementaire. Elles s'appuient en partie sur des **sources secondaires** (cabinets,
blogs, éditeurs concurrents). **Ce n'est pas un avis d'expert-comptable ni d'avocat.** Pour chaque mention ou règle écrite en
dur dans l'application : remplacer la référence par le texte consolidé sur Justel ou la circulaire sur Fisconetplus, et dater la
vérification. Rien de ce qui suit ne doit servir à des clients payants avant validation par un comptable et un avocat inscrit.

## Ce qui est fait (19/10/2026)

1. **Mentions légales de TVA** (`src/mentions_tva.rs`). Textes figés, jamais modifiables, imprimés en entier sur le PDF des
   factures et notes de crédit et repris dans l'UBL (cbc:Note) : autoliquidation (AR n° 1, art. 20, § 3) et taux de 6 % pour la
   rénovation d'un logement de plus de dix ans (question écrite n° 8-1 du Sénat). Une facture qui combine autoliquidation et 6 %
   est refusée à la validation. Remplace l'ancienne mention courte « TVA acquittée par autoliquidation ».
2. **Archive légale** (`src/archive.rs`, migration `0032`). Le PDF et l'UBL d'une facture validée sont fabriqués une fois, rangés
   avec leur empreinte SHA-256 et ne changent plus (déclencheur dans la base) ; téléchargement, e-mail, page client et export
   servent cette version. Conservation 10 ans à compter du 1er janvier suivant l'émission (art. 60 du Code TVA). Les factures
   reprises de Vertuoza n'ont pas d'archive (leur PDF d'origine est dans Vertuoza). Une facture déjà validée avant cette
   migration est archivée au premier usage (son archive reflète alors l'état du jour, pas celui de l'émission).
3. **Rappels de paiement** (`src/retard.rs`, migration `0033`). Un client sans numéro de TVA est un particulier (livre XIX du Code
   de droit économique) : rappel gratuit, délai de 14 jours calendrier à partir du lendemain de l'envoi, quatre mentions
   (créancier et numéro d'entreprise, facture et échéance, montant dû et montant de l'indemnité, délai). L'indemnité (barème 20 €
   / 30 € + 10 % / 65 € + 5 %, plafond 2 000 €) et les intérêts (plafonnés au taux de la loi du 2 août 2002) ne sont annoncés que
   si les réglages disent que les conditions générales les prévoient ; l'exception PME fait courir les intérêts dès le début du
   délai, à réclamer seulement après son expiration. Un client avec un numéro de TVA est une entreprise (loi du 2 août 2002 :
   intérêts légaux, 10,50 % au 2e semestre 2026, et 40 € d'indemnité forfaitaire). Le bloc légal est ajouté par le serveur après le
   message, il n'est pas dans la zone de texte. Un rappel ne part pas si la société n'a ni numéro d'entreprise ni e-mail ou
   téléphone. Réglages : Réglages › E-mails.
4. **Dossier de preuve de la signature** (`src/signature.rs`, migration `0034`). Avant de signer, le client reçoit un code à
   usage unique à l'adresse e-mail du devis (10 minutes, 5 essais, 3 envois par quart d'heure ; seule l'empreinte du code est
   gardée et il est masqué dans le journal). La signature écrit un dossier non modifiable : heure du serveur, nom, adresse e-mail
   et vérification (code par e-mail, mode test, sans e-mail), IP, navigateur, texte exact accepté, copie du PDF signé et son
   empreinte SHA-256. Un exemplaire signé est envoyé au client et téléchargeable. Le dossier est dans l'historique du devis.

## Choix faits par Claude, à valider

- **Mention du 6 %** : la mention standard (rénovation, logement de plus de dix ans) est imprimée pour **toute** ligne à 6 %. Les
  autres régimes à 6 % (pompes à chaleur dans un logement de moins de dix ans, démolition-reconstruction avec référence du
  formulaire 111) ont leur propre mention, **pas encore codée**.
- **Fin du texte du 6 %** : deux variantes circulent (« … le client endossera, par rapport à ces conditions, la responsabilité quant
  au paiement de la taxe… » et « … le client sera responsable du paiement de la taxe… »). On a retenu la première, qui suit la
  réponse du ministre. À copier mot pour mot depuis Fisconetplus, typographie (« 21% » ou « 21 % ») comprise.
- **Particulier ou entreprise** : décidé par la présence d'un numéro de TVA sur la fiche du client. Un client professionnel sans
  numéro enregistré est traité comme un particulier.
- **Taux d'intérêt commercial** : 1er semestre 2026 (fiche SPF Finances) et 2e semestre 2026 (calculé : 2,40 % + 8 points, arrondi
  à 10,50 % ; l'avis du Moniteur n'a pas été retrouvé). Au-delà de 2026, le taux n'est plus affiché (« taux légal en vigueur ») :
  **la table de `retard.rs` est à compléter chaque semestre.**
- **Rappels par courrier** : le calendrier du courrier (3e jour ouvrable, samedi compris) n'est pas géré, l'application n'envoie que des e-mails.
- **Adresse IP** : lue dans `X-Forwarded-For` (premier élément). À contrôler à la mise en ligne : un client peut écrire cet en-tête
  s'il atteint l'application sans passer par le serveur frontal de l'hébergeur.
- **Mode test** : sans clé Brevo, aucun code n'est envoyé et la signature reste possible, mais le dossier l'indique.
- **Pas d'horodatage qualifié** : l'heure est celle du serveur.

## À faire valider par un comptable / expert-comptable ITAA

1. Libellé exact de la mention de franchise de TVA et exclusion éventuelle des entreprises qui font des travaux immobiliers du
   régime de franchise (art. 56bis du Code TVA). Les sources divergent ; **rien n'est codé sur la franchise**.
2. Publication au Moniteur belge du seuil de franchise de 26 000 € pour 2027 (projet de loi « réduction des coûts II », DOC 56 1593).
3. Rubriques de l'AR n° 20 pour le 12 %, le logement social, les personnes handicapées, les régimes temporaires de 2026-2027 ;
   taux de 21 % depuis le 29/07/2025 pour la partie spécifique d'une installation de chauffage aux combustibles fossiles
   (circulaire 2025/C/47) : à contrôler sur les lots « Chauffage » des devis d'Ubique.
4. Base réglementaire de la mention des notes de crédit.
5. Avis du Moniteur pour le taux d'intérêt du 2e semestre 2026 et taux du 1er semestre 2027 dès sa publication.
6. Libellé commercial de la déduction de 120 % (circulaire 2026/C/46).
7. Format d'export préféré.
8. **Nos questions** : texte exact des deux mentions de TVA ; la ligne « TVA 0 % » doit-elle disparaître d'une facture en
   autoliquidation (le texte dit « pas de taux ni de montant de TVA ») ? ; numérotation annuelle des factures et série séparée des
   notes de crédit ; forme de la facture finale (récapitulatif du marché, acomptes déduits, référence aux factures antérieures,
   TVA de la finale limitée au solde) ; délai d'émission au 15 du mois suivant.

## À faire valider par un avocat inscrit

1. Portée et entrée en vigueur de la loi du 22 juillet 2026 modifiant la loi du 31 mai 2017 (décennale).
2. Exclusion « construction neuve / transformation importante », art. VI.53 et VI.73 du Code de droit économique, paiements
   pendant le délai de rétractation, transposition de la directive 2023/2673 (bouton de rétractation).
3. Rappels (livre XIX) : exception PME, plafonds, qualification de recouvreur amiable si le module s'élargit ; **texte du bloc
   légal des rappels**.
4. Clause de limitation de responsabilité du SaaS (art. VI.91/4 et VI.91/5 CDE, art. 5.89 du Code civil).
5. Clauses Data Act (art. 25 et 29) : frais de changement plafonnés au coût direct depuis le 12/09/2025, interdits depuis le 12/01/2027.
6. DPA et sous-traitants ultérieurs (hébergeur, Brevo, Stripe ou Mollie, point d'accès Peppol).
7. Base légale des pratiques commerciales trompeuses en B2B (ne jamais écrire « logiciel agréé ») et modèles de CG B2C des artisans.
8. **Nos questions** : une signature simple renforcée suffit-elle à 3 500 € ou plus face à un particulier (itsme recommandé) ;
   **phrase d'acceptation** affichée au client ; contenu et réglages des rappels.

## Pas encore fait

- Parcours de rétractation de 14 jours (information, formulaire type, bouton, case d'exécution anticipée non pré-cochée, question
  « avez-vous rencontré le client ? »). C'est le risque le plus lourd pour un devis signé à distance par un particulier.
- itsme ou signature qualifiée au-delà de 3 500 € TVAC face à un particulier ; horodatage qualifié.
- Peppol : envoi et réception (point d'accès certifié), validation Schematron avant envoi.
- Profil franchise de TVA, suivi du chiffre d'affaires, seuil par année ; table de taux datée par article.
- Mention décennale et attestation sur le devis ; alerte loi Breyne.
- Alerte du délai d'émission (15 du mois suivant) ; vérification du numéro de TVA du client (BCE, VIES).
- Mentions propres aux autres régimes à 6 % ; blocage du 6 % selon l'âge du bâtiment (à trancher : l'application ne doit pas
  choisir le taux à la place de l'artisan).
