-- E-mails (étape 8-2) : modèles modifiables et journal de tous les e-mails.
-- L'envoi passe par l'API de Brevo quand la clé BREVO_CLE_API est donnée au lancement du serveur ;
-- sans clé (et toujours pendant les tests), l'e-mail est seulement enregistré ici, rien ne part.

-- Les modèles, avec les variables entre crochets de Vertuoza ([NOM_CLIENT], [NUMBER_QUOTE], [NO_FACTURE]…).
CREATE TABLE modeles_email (
    code        text PRIMARY KEY CHECK (code IN ('devis', 'avenant', 'facture', 'note_credit', 'rappel_doux', 'rappel_ferme', 'export')),
    nom         text NOT NULL,
    position    integer NOT NULL,
    objet       text NOT NULL,
    corps       text NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);

INSERT INTO modeles_email (code, nom, position, objet, corps) VALUES
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

-- Le journal de tous les e-mails, envoyés ou seulement enregistrés.
CREATE TABLE emails (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    modele          text NOT NULL,
    devis_id        bigint REFERENCES devis (id) ON DELETE SET NULL,
    facture_id      bigint REFERENCES factures (id) ON DELETE SET NULL,
    destinataires   text NOT NULL,
    copie           text NOT NULL DEFAULT '',
    objet           text NOT NULL,
    corps           text NOT NULL,
    -- Noms des pièces jointes (les fichiers eux-mêmes ne sont pas gardés : ils se regénèrent).
    pieces          text[] NOT NULL DEFAULT '{}',
    -- enregistre : aucune clé Brevo, rien n'est parti ; envoye : accepté par Brevo ; echec : refusé par Brevo.
    statut          text NOT NULL CHECK (statut IN ('enregistre', 'envoye', 'echec')),
    erreur          text NOT NULL DEFAULT '',
    reference_brevo text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX emails_date ON emails (created_at DESC);
CREATE INDEX emails_devis ON emails (devis_id) WHERE devis_id IS NOT NULL;
CREATE INDEX emails_facture ON emails (facture_id) WHERE facture_id IS NOT NULL;

-- Le journal du devis note aussi les e-mails.
ALTER TABLE devis_evenements DROP CONSTRAINT devis_evenements_genre_check;
ALTER TABLE devis_evenements ADD CONSTRAINT devis_evenements_genre_check
    CHECK (genre IN ('creation', 'statut', 'pdf', 'duplication', 'chantier', 'email', 'client'));

-- Le lien secret de la page client en ligne (étape 9-1) : créé au premier envoi, impossible à deviner.
ALTER TABLE devis ADD COLUMN jeton text UNIQUE;
ALTER TABLE factures ADD COLUMN jeton text UNIQUE;
