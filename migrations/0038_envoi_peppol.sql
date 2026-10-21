-- Étape 5-2 : envoi des factures par Peppol, par le point d'accès Recommand.
--
-- L'organisation est inscrite chez Recommand comme « société » (une par organisation, toutes dans le compte de Raven).
-- Elle doit ensuite être vérifiée : un représentant de l'entreprise prouve son identité sur la page de Recommand
-- (le lien est gardé ici). Sans vérification, Recommand refuse tout envoi.
-- Envoi seulement pour l'instant : la société est créée sans inscription en réception (`isSmpRecipient: false`),
-- pour ne pas prendre la réception de factures que l'entreprise reçoit peut-être ailleurs (étape 5-3).
ALTER TABLE organisations
    ADD COLUMN peppol_societe_id text UNIQUE,
    ADD COLUMN peppol_verifiee boolean NOT NULL DEFAULT false,
    ADD COLUMN peppol_lien_verification text;

-- Chaque envoi d'une facture (ou note de crédit) validée. Un envoi en échec peut être refait : une ligne par essai.
CREATE TABLE envois_peppol (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organisation_id bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    facture_id      bigint NOT NULL,
    -- L'adresse Peppol du client (« 0208:0123456789 »).
    destinataire    text NOT NULL,
    -- L'identifiant du document chez Recommand (vide si l'envoi a été refusé avant d'être accepté).
    document_id     text NOT NULL DEFAULT '',
    statut          text NOT NULL CHECK (statut IN ('en_cours', 'livre', 'echec')),
    erreur          text NOT NULL DEFAULT '',
    envoye_le       timestamptz NOT NULL DEFAULT now(),
    maj_le          timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (organisation_id, facture_id) REFERENCES factures (organisation_id, id)
);
CREATE INDEX envois_peppol_facture ON envois_peppol (facture_id, envoye_le);
ALTER TABLE envois_peppol ENABLE ROW LEVEL SECURITY;
ALTER TABLE envois_peppol FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON envois_peppol USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());

-- L'historique de la facture note les envois par Peppol.
ALTER TABLE facture_evenements DROP CONSTRAINT facture_evenements_genre_check,
    ADD CONSTRAINT facture_evenements_genre_check
        CHECK (genre IN ('validation', 'envoi', 'paiement', 'paiement_supprime', 'email', 'rappel', 'client', 'peppol'));
