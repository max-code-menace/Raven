-- Raven, étape 1-3 : ce que reçoit une nouvelle organisation.
-- Les modèles d'e-mails de départ, communs à tout le site et jamais modifiés par une organisation : à la création,
-- chaque organisation en reçoit une copie, qu'elle modifie ensuite dans Réglages › E-mails (voir
-- `organisations::creer`). Ce sont les textes neutres d'origine (migration 0015), pas ceux qu'Ubique a repris de
-- Vertuoza.

CREATE TABLE modeles_email_defaut (
    code        text PRIMARY KEY CHECK (code IN ('devis', 'avenant', 'facture', 'note_credit', 'rappel_doux', 'rappel_ferme', 'export')),
    nom         text NOT NULL,
    position    integer NOT NULL,
    objet       text NOT NULL,
    corps       text NOT NULL
);

INSERT INTO modeles_email_defaut (code, nom, position, objet, corps) VALUES
('devis', 'Devis', 1, 'Devis n° [NUMBER_QUOTE] - [REFERENCE]',
'Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Veuillez trouver en pièce jointe notre devis n° [NUMBER_QUOTE] ([REFERENCE]).
Vous pouvez aussi le consulter et le signer en ligne : [LINK_DEVIS]

Nous restons à votre disposition pour toute question.

Bien à vous,
[NOM_GESTIONNAIRE]
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]'),
('avenant', 'Avenant', 2, 'Avenant n° [NUMBER_QUOTE] - [NOM_CHANTIER]',
'Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Veuillez trouver en pièce jointe l''avenant n° [NUMBER_QUOTE] au devis du chantier [NOM_CHANTIER].
Vous pouvez aussi le consulter et le signer en ligne : [LINK_DEVIS]

Bien à vous,
[NOM_GESTIONNAIRE]
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]'),
('facture', 'Facture', 3, 'Facture n° [NO_FACTURE] - [REFERENCE]',
'Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Veuillez trouver en pièce jointe notre facture n° [NO_FACTURE] du [DATE_FACTURE], d''un montant de [MONTANT_TTC].
Merci de la régler pour le [ECHEANCE] sur le compte [IBAN] avec la communication [COMMUNICATION].
Vous pouvez aussi la consulter et la payer par QR code en ligne : [LINK_FACTURE]

Bien à vous,
[NOM_GESTIONNAIRE]
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]'),
('note_credit', 'Note de crédit', 4, 'Note de crédit n° [NO_FACTURE] - [REFERENCE]',
'Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Veuillez trouver en pièce jointe notre note de crédit n° [NO_FACTURE] du [DATE_FACTURE], d''un montant de [MONTANT_TTC].

Bien à vous,
[NOM_GESTIONNAIRE]
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]'),
('rappel_doux', 'Rappel de paiement (version douce)', 5, 'Oubli de paiement ? Facture n° [NO_FACTURE]',
'Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Sauf erreur de notre part, la facture n° [NO_FACTURE] du [DATE_FACTURE] (échéance le [ECHEANCE]) n''est pas encore réglée.
Reste à payer : [RESTE_A_PAYER], sur le compte [IBAN] avec la communication [COMMUNICATION].
Vous pouvez aussi la payer par QR code en ligne : [LINK_FACTURE]

Si le paiement est déjà parti, merci de ne pas tenir compte de ce message.

Bien à vous,
[NOM_GESTIONNAIRE]
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]'),
('rappel_ferme', 'Rappel de paiement (version ferme)', 6, 'Rappel de paiement : facture n° [NO_FACTURE]',
'Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Malgré notre précédent rappel, la facture n° [NO_FACTURE] du [DATE_FACTURE] (échéance le [ECHEANCE]) reste impayée.
Nous vous prions de régler [RESTE_A_PAYER] sans délai sur le compte [IBAN] avec la communication [COMMUNICATION].
Paiement par QR code en ligne : [LINK_FACTURE]

Bien à vous,
[NOM_GESTIONNAIRE]
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]'),
('export', 'Export pour le comptable', 7, 'Factures [NOM_SOCIETE] : [PERIODE]',
'Bonjour,

Veuillez trouver en pièce jointe les factures et notes de crédit de [NOM_SOCIETE] pour la période [PERIODE].

Bien à vous,
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]');
