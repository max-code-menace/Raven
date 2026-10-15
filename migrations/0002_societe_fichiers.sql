-- Paramètres société : plusieurs e-mails et téléphones, et les deux fichiers (logo, conditions
-- générales) rangés dans la base pour qu'ils soient sauvegardés avec le reste.

ALTER TABLE societe DROP COLUMN conditions_generales;
ALTER TABLE societe
    ADD COLUMN emails_supplementaires     text[] NOT NULL DEFAULT '{}',
    ADD COLUMN telephones_supplementaires text[] NOT NULL DEFAULT '{}';

CREATE TABLE societe_fichiers (
    usage       text PRIMARY KEY CHECK (usage IN ('logo', 'conditions_generales')),
    nom         text NOT NULL,
    type_mime   text NOT NULL,
    contenu     bytea NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);
