-- Étape 5-3 : réception des factures fournisseurs par Peppol (décision de Mahx du 21/10/2026).
--
-- La réception est une option par organisation : une entreprise ne reçoit que par un seul point d'accès, la prendre
-- sans prévenir priverait son comptable ou son autre logiciel de ses factures. Elle ne s'allume que sur un clic du
-- propriétaire (Réglages › Peppol), qui inscrit l'entreprise en réception chez Recommand (`isSmpRecipient: true`).
ALTER TABLE organisations
    ADD COLUMN peppol_reception boolean NOT NULL DEFAULT false,
    -- La dernière relève de la boîte de réception chez Recommand.
    ADD COLUMN peppol_releve_le timestamptz;

-- Une facture (ou note de crédit) reçue d'un fournisseur. Le XML reçu est gardé tel quel (preuve, comme l'archive des
-- ventes) avec son empreinte SHA-256 ; ce que Recommand en a lu (`donnees`) sert à l'afficher. Rien de ce qui vient du
-- fournisseur ne se modifie ; seuls changent le lien vers sa fiche et la part classée sans chantier (frais généraux).
-- Montants comme sur le document (positifs, aussi pour une note de crédit : elle compte en négatif dans les achats).
CREATE TABLE factures_recues (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organisation_id     bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    -- L'identifiant du document chez Recommand.
    document_id         text NOT NULL,
    genre               text NOT NULL CHECK (genre IN ('facture', 'note_credit')),
    -- L'adresse Peppol de l'expéditeur (« 0208:0123456789 »).
    expediteur          text NOT NULL DEFAULT '',
    -- La fiche du fournisseur dans les entreprises (retrouvée par son numéro de TVA, ou créée à l'arrivée).
    fournisseur_id      bigint,
    fournisseur_nom     text NOT NULL,
    fournisseur_tva     text NOT NULL DEFAULT '',
    numero              text NOT NULL DEFAULT '',
    date_facture        date NOT NULL,
    date_echeance       date,
    montant_ht_cents    bigint NOT NULL,
    montant_tva_cents   bigint NOT NULL,
    montant_ttc_cents   bigint NOT NULL,
    a_payer_cents       bigint NOT NULL,
    iban                text NOT NULL DEFAULT '',
    communication       text NOT NULL DEFAULT '',
    -- Le résultat du contrôle de Recommand (« valid », « invalid »…), vide s'il n'y en a pas eu.
    validation          text NOT NULL DEFAULT '',
    donnees             jsonb NOT NULL,
    xml                 text NOT NULL,
    empreinte           text NOT NULL CHECK (length(empreinte) = 64),
    -- Le PDF joint par le fournisseur, sinon le rendu de Recommand (absent si aucun des deux).
    pdf                 bytea,
    recue_le            timestamptz NOT NULL,
    enregistree_le      timestamptz NOT NULL DEFAULT now(),
    -- La part de la facture classée sans chantier (frais généraux), HT, avec le signe de la pièce.
    frais_generaux_cents bigint NOT NULL DEFAULT 0,
    UNIQUE (organisation_id, document_id),
    UNIQUE (organisation_id, id),
    FOREIGN KEY (organisation_id, fournisseur_id) REFERENCES entreprises (organisation_id, id) ON DELETE SET NULL (fournisseur_id)
);
CREATE INDEX factures_recues_date ON factures_recues (organisation_id, date_facture);
ALTER TABLE factures_recues ENABLE ROW LEVEL SECURITY;
ALTER TABLE factures_recues FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON factures_recues USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());

CREATE FUNCTION factures_recues_proteger() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'une facture reçue ne se supprime pas';
    END IF;
    IF (NEW.organisation_id, NEW.document_id, NEW.genre, NEW.expediteur, NEW.fournisseur_nom, NEW.fournisseur_tva, NEW.numero,
        NEW.date_facture, NEW.date_echeance, NEW.montant_ht_cents, NEW.montant_tva_cents, NEW.montant_ttc_cents, NEW.a_payer_cents,
        NEW.iban, NEW.communication, NEW.validation, NEW.donnees, NEW.xml, NEW.empreinte, NEW.pdf, NEW.recue_le, NEW.enregistree_le)
       IS DISTINCT FROM
       (OLD.organisation_id, OLD.document_id, OLD.genre, OLD.expediteur, OLD.fournisseur_nom, OLD.fournisseur_tva, OLD.numero,
        OLD.date_facture, OLD.date_echeance, OLD.montant_ht_cents, OLD.montant_tva_cents, OLD.montant_ttc_cents, OLD.a_payer_cents,
        OLD.iban, OLD.communication, OLD.validation, OLD.donnees, OLD.xml, OLD.empreinte, OLD.pdf, OLD.recue_le, OLD.enregistree_le) THEN
        RAISE EXCEPTION 'une facture reçue ne se modifie pas';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER factures_recues_immuables BEFORE UPDATE OR DELETE ON factures_recues
    FOR EACH ROW EXECUTE FUNCTION factures_recues_proteger();

-- Rattacher une facture reçue à un chantier en fait un achat du chantier (budget et rentabilité à jour). Une facture
-- peut être répartie sur plusieurs chantiers : un achat par part.
ALTER TABLE chantier_achats
    ADD COLUMN facture_recue_id bigint,
    ADD CONSTRAINT chantier_achats_facture_recue_id_fkey
        FOREIGN KEY (organisation_id, facture_recue_id) REFERENCES factures_recues (organisation_id, id);
CREATE INDEX chantier_achats_facture_recue ON chantier_achats (facture_recue_id) WHERE facture_recue_id IS NOT NULL;

-- Les factures reçues de chaque export pour le comptable (choix « uniquement non exportées »), à part des ventes.
CREATE TABLE export_factures_recues (
    organisation_id     bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    export_id           bigint NOT NULL,
    facture_recue_id    bigint NOT NULL,
    PRIMARY KEY (export_id, facture_recue_id),
    FOREIGN KEY (organisation_id, export_id) REFERENCES exports (organisation_id, id) ON DELETE CASCADE,
    FOREIGN KEY (organisation_id, facture_recue_id) REFERENCES factures_recues (organisation_id, id)
);
CREATE INDEX export_factures_recues_facture ON export_factures_recues (facture_recue_id);
ALTER TABLE export_factures_recues ENABLE ROW LEVEL SECURITY;
ALTER TABLE export_factures_recues FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON export_factures_recues USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());
