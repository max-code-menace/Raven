# Étape 5 : comparaison des points d'accès Peppol (20/10/2026)

Recherche faite par Claude le 20/10/2026, **depuis des moteurs de recherche seulement** (les sites des prestataires n'ont pas pu être ouverts directement depuis l'environnement de travail). Les prix sont ceux publiés ou relevés par des comparatifs ; plusieurs comparatifs sont écrits par un concurrent. **Tout est à confirmer auprès du prestataire avant de signer.**

## Ce dont Raven a besoin
- **Envoyer** les factures et notes de crédit en UBL (Peppol BIS Billing 3.0) : Raven les fabrique déjà (`src/ubl.rs`). Il nous faut donc une API qui accepte **notre propre XML**, pas un outil de facturation de plus.
- **Plusieurs organisations** (nos clients) derrière un seul compte Raven, sans coût fixe par organisation.
- Suivi du statut d'envoi (idéalement par webhook), environnement de test.
- Prestataire certifié OpenPeppol, hébergé en UE, contrat de sous-traitance (RGPD).
- Réception : **après la V1** (mais un prestataire qui sait la faire évite de changer plus tard).

Ordre de grandeur pour chiffrer : **100 organisations × 15 factures Peppol par mois = 1 500 documents par mois** (les factures aux particuliers ne passent pas par Peppol).

## Tableau

| Prestataire | Pays | Modèle de prix publié | Coût estimé à 1 500 doc./mois | API pour éditeurs | Remarques |
|---|---|---|---|---|---|
| **Recommand** (recommand.eu) | BE | Gratuit 25 doc./mois (0,30 € le suivant) ; 29 €/mois pour 200 (0,20 €) ; 99 €/mois pour 1 000 (0,10 €) ; 199 €/mois pour 2 500 (0,08 €) ; 349 €/mois pour 5 000 (0,07 €). Sans frais d'entrée ni engagement | **≈ 150 à 200 €/mois** | REST, plusieurs sociétés par équipe **sans supplément** (volume commun), envoi et réception comptés pareil | Code **open source (AGPL-3.0)** : on pourrait l'héberger nous-mêmes en dernier recours. On fournit notre propre UBL (ce qu'on fait déjà) |
| **e-invoice.be** | BE | Pro 0,25 €/doc. (paquets dès 350 €) ; Enterprise 0,18 €/doc. avec remises de volume. Sans frais d'entrée ni minimum mensuel | ≈ 270 à 375 €/mois | REST, OpenAPI, SDK (JS, Python, Ruby, Java, PHP), bac à sable gratuit | Envoi et réception facturés. Pas de SDK Rust (pas grave : appels HTTP avec `ureq`, comme Brevo) |
| **Digiteal** | BE | Intégrateur : 365 € HT une fois, puis prix par document (non publié), **minimum 85 € HT/mois** payé d'avance | ≥ 85 €/mois + documents (devis) | API seule, webhooks, n'accepte que de l'UBL BIS 3.0 | Contrat signé obligatoire |
| **Qvalia** | SE | Partenaires : 50 €/mois + 0,05 €/message envoyé ou reçu (conditions par contrat) | ≈ 125 €/mois | API pour partenaires | Le moins cher sur le papier ; société suédoise |
| **Peppyrus** | BE | Gratuit : 100 envois/mois, réception illimitée ; offres payantes annoncées **au 1er novembre 2026** (prix inconnus) | ? | API + environnement de test (réseau de test Peppol) | Gratuit pensé pour une seule entreprise ; conditions pour un éditeur à demander |
| **Storecove** | NL | Sur devis ; souvent cité « dès ≈ 495 €/mois » (non confirmé) ; prix liés au nombre d'entités juridiques | ≈ 500 €/mois ou plus | API de référence pour les éditeurs, nombreux pays, 30 jours d'essai | Le plus complet, le plus cher, facturation par entité défavorable à beaucoup de petites organisations |
| **Scrada** | BE | Version « Peppol only » pour éditeurs : **sur demande** (info@scrada.be) ; paiement à l'usage | ? | API, faite pour les éditeurs qui envoient pour leurs clients | À demander |
| **Billit** | BE | Logiciel : gratuit pour envoyer, puis 7,50 € à 145 €/mois ; point d'accès pour éditeurs : **prix non publié** | ? | API REST, mais clé « non commerciale » ; contrat éditeur à part | Concurrent direct de Raven (logiciel de facturation) |

Écartés : **Hermes / Mercurius** (plateformes publiques, sans API pour un éditeur), **Odoo** (gratuit seulement dans Odoo), **Let's Peppol** (boîte gratuite pour indépendants, pas d'API publique), **monter notre propre point d'accès** (Oxalis, phase4 : certification OpenPeppol, audits, AS4 et certificats à maintenir : disproportionné pour Raven).

## Coût pour Raven
Entre **0,07 € et 0,25 € par facture**. Une organisation qui envoie 15 factures Peppol par mois coûte donc 1 à 4 € par mois, à comparer aux 35 ou 59 € de l'abonnement : **Peppol peut rester inclus dans les deux formules**.

## Point à ne pas rater : l'enregistrement dans l'annuaire Peppol
Sur Peppol, un numéro d'entreprise ne peut être inscrit comme **destinataire** (pour un type de document) que chez **un seul** point d'accès. Beaucoup de nos clients seront déjà inscrits ailleurs (chez leur comptable, Billit, Hermes…) pour recevoir leurs factures.
- Pour la V1 (envoi seulement), Raven ne doit **pas** inscrire l'organisation comme destinataire, sinon il lui « vole » sa réception. À vérifier chez le prestataire : envoi possible sans inscription en réception.
- Le point d'accès doit vérifier l'identité de l'entreprise qui envoie (KYC). À demander : comment cela se passe pour nos clients (automatique par la BCE, pièce d'identité, itsme ?) et si cela peut se faire **en libre-service** depuis Raven (principe du produit).

## Recommandation de Claude
1. **Recommand en premier choix** : belge, prix publiés et les plus bas à notre volume, toutes nos organisations dans un seul compte sans supplément, API faite pour envoyer son propre UBL, code ouvert (on ne reste pas coincé si le prestataire disparaît).
2. **e-invoice.be en solution de rechange** : belge, prix simples sans minimum, bonne documentation ; environ deux fois plus cher.
3. Demander aussi un devis à **Digiteal** et **Scrada** (belges, faits pour les éditeurs) pour comparer.

Le code de Raven sera écrit pour qu'un changement de prestataire ne touche qu'un seul fichier (comme `emails.rs` pour Brevo).

## Questions à poser aux prestataires retenus (avant de choisir)
1. Êtes-vous un point d'accès **certifié OpenPeppol** (et sous quelle autorité) ?
2. Prix pour un éditeur avec **plusieurs centaines d'organisations** : coût par organisation, par document, minimum mensuel, frais d'entrée ?
3. **Envoi sans inscription en réception** dans l'annuaire ? Que se passe-t-il si l'organisation est déjà inscrite chez un autre point d'accès ?
4. **Vérification d'identité** (KYC) de nos clients : qui la fait, comment, en combien de temps, par API ?
5. Validation des règles Peppol (Schematron) **avant** l'envoi, et messages d'erreur renvoyés ?
6. Statuts d'envoi : **webhooks** ? Accusé de réception du destinataire (Message Level Response) ?
7. **Environnement de test** gratuit relié au réseau de test Peppol ?
8. Hébergement **en UE**, contrat de sous-traitance (RGPD), durée de conservation des documents chez eux ?
9. La **franchise de TVA** : catégorie « E » avec le motif « Régime particulier de franchise des petites entreprises. » acceptée ? (question ouverte depuis l'étape 3)
10. Réception plus tard : même contrat, même API ?

## Sources
- Recommand : [tarifs](https://recommand.eu/en/pricing), [FAQ](https://docs.recommand.eu/faq), [code source](https://github.com/brbxai/recommand-peppol)
- e-invoice.be : [API](https://e-invoice.be/peppol-api), [comparatif (écrit par e-invoice.be)](https://e-invoice.be/blog/best-peppol-access-points), [face à Storecove](https://e-invoice.be/alternatives/storecove), [GitHub](https://github.com/e-invoice-be)
- Digiteal : [offres](https://www.digiteal.eu/?p=26629), [documentation du point d'accès](https://doc.digiteal.eu/docs/peppol-access-point)
- Qvalia : [prix partenaires](https://qvalia.com/help/pricing-model-for-peppol-partners/)
- Peppyrus : [site](https://www.peppyrus.be/en), [liste des plateformes belges (Dexxter)](https://dexxter.be/en/list-of-peppol-platforms-in-belgium/)
- Storecove : [G2](https://www.g2.com/products/storecove/pricing), [RFP.wiki](https://www.rfp.wiki/accounting-finance/accounts-payable-applications/e-invoicing-compliance-applications/storecove)
- Scrada : [API Peppol](https://www.scrada.be/en/peppol/), [support (prix sur demande)](https://support.scrada.be/nl/support/solutions/articles/103000349973-peppol-api-informatie)
- Billit : [point d'accès](https://www.billit.eu/en-int/peppol-access-point/), [brochure](https://www.billit.eu/media/g0xnh0t4/202508_folder_billit_access_point_en_digital.pdf)
- Comparatifs : [WiseTREND, 80 prestataires (09/2026)](https://www.wisetrend.com/blog/cost-of-e-invoices-global-marketplace-research-competitive-offer/), [getpeppr](https://getpeppr.dev/best-peppol-api/)
- Vérification d'identité : [Banqup](https://www.banqup.com/fr-be/resources/blog/prevenir-la-fraude-sur-les-factures) ; logiciels libres : [OpenPeppol](https://peppol.org/tools-support/links-to-software/) ; Let's Peppol : [ITdaily](https://itdaily.com/news/software/limburg-association-free-peppol-mailbox)
