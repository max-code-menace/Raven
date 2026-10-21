# Étape 5 : comparaison des points d'accès Peppol (20/10/2026)

Recherche faite par Claude le 20/10/2026. Première version écrite depuis des moteurs de recherche, puis **vérifiée sur les sites des prestataires** le même jour (pages de prix, documentation de l'API, liste officielle d'OpenPeppol). Ce qui n'a pas pu être vérifié est marqué « non publié » ou « à demander ». Les prix changent : à reconfirmer au moment de signer.

## Ce dont Raven a besoin
- **Envoyer** les factures et notes de crédit en UBL (Peppol BIS Billing 3.0) : Raven les fabrique déjà (`src/ubl.rs`). Il nous faut donc une API qui accepte **notre propre XML**, pas un outil de facturation de plus.
- **Plusieurs organisations** (nos clients) derrière un seul compte Raven, sans coût fixe par organisation.
- Suivi du statut d'envoi (idéalement par webhook), environnement de test.
- Prestataire certifié OpenPeppol, hébergé en UE, contrat de sous-traitance (RGPD).
- Réception : **dans la V1** (décision de Mahx du 21/10/2026) : Ubique reçoit ses factures fournisseurs dans Vertuoza et les rattache à ses chantiers.

Ordre de grandeur pour chiffrer : **100 organisations × 15 factures Peppol par mois = 1 500 documents par mois** (les factures aux particuliers ne passent pas par Peppol).

## Certification
Les huit prestataires du tableau figurent sur la liste officielle d'OpenPeppol (« Peppol Certified Service Providers », relevée le 20/10/2026), tous « AP Certified » et « SMP Certified » : Recommand (BRBX BV, autorité belge BOSA), e-invoice.be (BOSA), Digiteal SA, Scrada BV, Billit, Tigron BV (Peppyrus), Qvalia AB, Storecove (Datajust B.V.).

## Tableau

| Prestataire | Pays | Prix (vérifié sur le site sauf mention) | Coût estimé à 1 500 doc./mois | API pour éditeurs | Remarques |
|---|---|---|---|---|---|
| **Recommand** (recommand.eu) | BE | Gratuit 25 doc./mois (0,30 € le suivant) ; 29 €/mois pour 200 (0,20 €) ; 99 €/mois pour 1 000 (0,10 €) ; 199 €/mois pour 2 500 (0,08 €) ; 349 €/mois pour 5 000 (0,07 €) ; au-delà sur devis. Sans frais d'entrée, sans engagement, mensuel | **≈ 150 €/mois** (formule 1 000 + 500 × 0,10 €) | REST (JSON ou **UBL brut**), **sociétés illimitées dans une équipe, sans frais par société** (volume commun), webhooks signés, bacs à sable gratuits (aussi reliés au réseau de test Peppol), marque blanche permise | Envoi et réception comptés pareil. Archivage 1 an inclus (formule 1 000). Code **open source (AGPL-3.0)** |
| **e-invoice.be** | BE | Pro 0,25 €/facture (paquets dès 350 €) ; Enterprise 0,18 €/facture, remises de volume | ≈ 270 à 375 €/mois | REST avec webhooks, SDK (JS, Python, Ruby, Java, PHP), envoi par e-mail aussi | Données en UE. Accès « multi-équipes » seulement dans Enterprise ; conditions pour un éditeur à demander |
| **Digiteal** | BE | Intégrateur : 365 € HT une fois, prix par document envoyé ou reçu **non publié**, **minimum 85 € HT/mois** payé d'avance, 0,5 à 1 jour d'accompagnement | ≥ 85 €/mois + documents (devis) | API seule (« Digiteal invisible »), n'accepte que de l'UBL BIS 3.0 | Contrat signé obligatoire |
| **Qvalia** | SE | Exemple publié pour les partenaires : 50 €/mois + 0,05 €/message envoyé ou reçu ; conditions finales par contrat | ≈ 125 €/mois | API partenaire multi-clients, marque blanche possible | Le moins cher sur le papier ; société suédoise ; contrat à négocier |
| **Peppyrus** (Tigron) | BE | Nouvelles formules au **1er novembre 2026** : gratuit 100 envois/mois ; 15 €/mois pour 250 ; 49 €/mois pour 1 000 ; 149 €/mois envois illimités ; réception illimitée partout. Intégrations sur mesure : par Tigron | ? (si une seule licence couvre toutes nos organisations : 149 €/mois) | API + environnement de test relié au réseau de test Peppol | Formules pensées **par entreprise** ; le cas d'un éditeur avec des centaines d'organisations n'est pas décrit : à demander à Tigron |
| **Storecove** | NL | **Aucun prix publié** sur le site (le « dès 495 €/mois » des comparatifs n'est pas confirmé) | ? | API de référence, nombreux pays, bac à sable 30 jours | Sur devis ; prix liés au nombre d'entités juridiques selon des comparatifs |
| **Scrada** | BE | API « Peppol Only » : **prix sur demande**, vendue seulement aux développeurs | ? | Envoi depuis n'importe quelle société (JSON converti en UBL ; envoi d'UBL brut à confirmer), statut par webhook, inscription en réception par API, autofacturation | UBL envoyé gardé 3 mois seulement |
| **Billit** | BE | Prix pour un éditeur qui relie son logiciel au point d'accès : **non publié** | ? | Liaison d'un logiciel au point d'accès Billit annoncée | Concurrent direct de Raven (logiciel de facturation) |

Écartés : **Hermes / Mercurius** (plateformes publiques, sans API pour un éditeur), **Odoo** (gratuit seulement dans Odoo), **Let's Peppol** (boîte gratuite pour indépendants, pas d'API publique), **monter notre propre point d'accès** (Oxalis, phase4 : certification OpenPeppol, audits, AS4 et certificats à maintenir : disproportionné pour Raven).

## Coût pour Raven
Entre **0,07 € et 0,25 € par facture**. Une organisation qui envoie 15 factures Peppol par mois coûte donc 1 à 4 € par mois, à comparer aux 35 ou 59 € de l'abonnement : **Peppol peut rester inclus dans les deux formules**. Recommand et e-invoice.be annoncent que ces frais sont déductibles à 120 % pour une société belge (déduction majorée pour la facturation électronique : à confirmer avec le comptable pour la société qui vendra Raven).

## Point à ne pas rater : l'inscription en réception
Sur Peppol, une entreprise ne peut être inscrite comme **destinataire** (pour un type de document) que chez **un seul** point d'accès (confirmé par Recommand et Scrada). Beaucoup de nos clients seront déjà inscrits ailleurs (chez leur comptable, Billit, Hermes…) pour recevoir leurs factures.
- Pour la V1 (envoi seulement), Raven ne doit **pas** inscrire l'organisation comme destinataire, sinon il lui prend sa réception. Scrada le confirme : l'inscription n'est nécessaire que pour recevoir, pas pour envoyer. Chez Recommand : réglage `isSmpRecipient: false` à la création de la société (« sending only »).
- La société qui envoie doit quand même **prouver son identité** (KYC). Chez Recommand, c'est en **libre-service** : la création de la société par l'API renvoie un lien de vérification (`verificationUrl`) que Raven montre à l'utilisateur ; un représentant autorisé fait le contrôle d'identité dans son navigateur (le lien peut être transmis à la bonne personne), sans compte Recommand ; Raven est prévenu par le webhook `company.verification` (vérifiée, refusée, erreur). Changer le numéro de TVA ou d'entreprise oblige à refaire la vérification.
- Réception (dans la V1, en option par organisation) : `isSmpRecipient: true`, factures reçues par le webhook `document.received`. Pour une organisation déjà inscrite ailleurs (Ubique chez Vertuoza) : migration des identifiants depuis l'ancien prestataire par des « migration keys » (ou désinscription chez l'ancien, avec un trou de réception entre les deux).

## Recommandation de Claude
1. **Recommand en premier choix** : belge, certifié, prix publiés et les plus bas parmi ceux qui sont publiés à notre volume (≈ 150 €/mois pour 1 500 documents), toutes nos organisations dans un seul compte sans frais par organisation, envoi de **notre propre UBL**, envoi seul sans prendre la réception du client, vérification d'identité en libre-service (principe du produit), webhooks, bac à sable gratuit, code ouvert (on ne reste pas coincé si le prestataire disparaît). On peut ouvrir un compte et un bac à sable aujourd'hui, sans contrat.
2. **e-invoice.be en solution de rechange** : belge, certifié, prix simples sans minimum, bonne documentation ; environ deux fois plus cher.
3. Pour comparer, demander un devis à **Qvalia** (le moins cher sur le papier, à négocier) et à **Tigron/Peppyrus** (149 €/mois illimités, si cela couvre un éditeur).

Le code de Raven sera écrit pour qu'un changement de prestataire ne touche qu'un seul fichier (comme `emails.rs` pour Brevo).

## Ce qui reste à confirmer
Déjà répondu par la documentation de Recommand : certification, envoi sans inscription en réception, vérification d'identité par API, webhooks, bac à sable, envoi d'UBL brut, plusieurs sociétés sans supplément, communication structurée (format numérique, sans « +++ » ni « / »).

À demander (à Recommand en premier, aux autres si on veut comparer) :
1. Validation des règles Peppol (Schematron) **avant** l'envoi, et forme des messages d'erreur renvoyés ?
2. Accusé de réception du destinataire (Message Level Response, refus d'une facture) ?
3. Hébergement **en UE**, contrat de sous-traitance (RGPD), durée de conservation des documents chez eux (1 an d'archive annoncé) ?
4. La **franchise de TVA** : catégorie « E » avec le motif « Régime particulier de franchise des petites entreprises. » acceptée ? (question ouverte depuis l'étape 3)
5. Contrat pour un éditeur (garanties de service, préavis en cas d'arrêt, changement de prix) ?
6. Pour Peppyrus : une licence couvre-t-elle plusieurs centaines d'organisations ? Pour Qvalia, Digiteal, Scrada, Storecove : prix réel pour notre volume.

## Sources (consultées le 20/10/2026)
- Liste officielle : [OpenPeppol, Peppol Certified Service Providers](https://peppol.org/members/peppol-certified-service-providers/)
- Recommand : [tarifs](https://recommand.eu/en/pricing), [FAQ](https://docs.recommand.eu/faq), [gestion des sociétés](https://docs.recommand.eu/docs/managing-companies), [vérification des sociétés](https://docs.recommand.eu/docs/company-verification), [webhooks](https://docs.recommand.eu/docs/working-with-webhooks), [code source](https://github.com/recommand/recommand-peppol)
- e-invoice.be : [tarifs](https://e-invoice.be/pricing), [API](https://e-invoice.be/peppol-api)
- Digiteal : [offres](https://www.digiteal.eu/?p=26629)
- Qvalia : [prix partenaires](https://qvalia.com/help/pricing-model-for-peppol-partners/)
- Peppyrus : [site et licences](https://www.peppyrus.be/en)
- Storecove : [devenir intégrateur](https://www.storecove.com/us/en/solutions/become-an-integrator)
- Scrada : [API Peppol](https://www.scrada.be/en/peppol/)
- Billit : [point d'accès](https://www.billit.eu/en-int/peppol-access-point/)
- Comparatifs consultés au départ (non indépendants pour certains) : [WiseTREND (09/2026)](https://www.wisetrend.com/blog/cost-of-e-invoices-global-marketplace-research-competitive-offer/), [e-invoice.be](https://e-invoice.be/blog/best-peppol-access-points), [getpeppr](https://getpeppr.dev/best-peppol-api/)
