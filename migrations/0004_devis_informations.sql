-- Devis : champs de l'onglet Informations de l'éditeur (étape 5b-1).
-- Les lignes et les totaux arrivent aux étapes suivantes.

ALTER TABLE devis
    -- Membre de l'entreprise qui suit le devis (texte libre : il n'y a pas encore de comptes).
    ADD COLUMN responsable                  text NOT NULL DEFAULT '',
    ADD COLUMN taux_tva_id                  bigint REFERENCES taux_tva (id),
    ADD COLUMN conditions_paiement_id       bigint REFERENCES conditions_paiement (id),
    -- Facturé au client du devis, ou à un autre client (assurance, syndic…).
    ADD COLUMN facturation_identique        boolean NOT NULL DEFAULT true,
    ADD COLUMN facturation_entreprise_id    bigint REFERENCES entreprises (id),
    ADD COLUMN facturation_contact_id       bigint REFERENCES contacts (id),
    -- Adresse du chantier : celle du client, ou une adresse propre au devis.
    ADD COLUMN chantier_identique           boolean NOT NULL DEFAULT true,
    ADD COLUMN chantier_rue                 text NOT NULL DEFAULT '',
    ADD COLUMN chantier_numero              text NOT NULL DEFAULT '',
    ADD COLUMN chantier_code_postal         text NOT NULL DEFAULT '',
    ADD COLUMN chantier_ville               text NOT NULL DEFAULT '',
    ADD COLUMN chantier_pays                text NOT NULL DEFAULT 'BE',
    ADD COLUMN architecte_contact_id        bigint REFERENCES contacts (id),
    ADD COLUMN gestionnaire_contact_id      bigint REFERENCES contacts (id),
    -- Remarque 1 = conditions particulières, remarque 2 = libre.
    ADD COLUMN remarque_1                   text NOT NULL DEFAULT '',
    ADD COLUMN remarque_2                   text NOT NULL DEFAULT '',
    ADD CONSTRAINT devis_un_seul_client_facturation
        CHECK (facturation_entreprise_id IS NULL OR facturation_contact_id IS NULL),
    ADD CONSTRAINT devis_facturation_identique_sans_autre_client
        CHECK (NOT facturation_identique OR (facturation_entreprise_id IS NULL AND facturation_contact_id IS NULL));
