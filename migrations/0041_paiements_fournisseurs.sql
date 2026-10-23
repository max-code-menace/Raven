-- Paiements des factures fournisseurs reçues (23/10/2026, priorité 2 de la comparaison avec Vertuoza, où une facture
-- fournisseur passe de « À valider » à « À payer », « Part. payé » puis « Payé »). Comme pour les factures clients, le
-- statut de paiement n'est pas enregistré : la vue `factures_recues_etats` le calcule depuis les paiements et l'échéance.
-- Une note de crédit fournisseur ne se paie pas.
CREATE TABLE paiements_fournisseurs (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organisation_id     bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    facture_recue_id    bigint NOT NULL,
    date_paiement       date NOT NULL,
    montant_cents       bigint NOT NULL CHECK (montant_cents > 0),
    methode_id          bigint NOT NULL REFERENCES methodes_paiement (id),
    remarque            text NOT NULL DEFAULT '' CHECK (length(remarque) <= 500),
    cree_le             timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organisation_id, id),
    FOREIGN KEY (organisation_id, facture_recue_id) REFERENCES factures_recues (organisation_id, id)
);
CREATE INDEX paiements_fournisseurs_facture ON paiements_fournisseurs (facture_recue_id);
ALTER TABLE paiements_fournisseurs ENABLE ROW LEVEL SECURITY;
ALTER TABLE paiements_fournisseurs FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON paiements_fournisseurs USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());

-- Second filet (l'écran vérifie déjà) : pas de paiement sur une note de crédit, jamais plus que le montant à payer.
CREATE FUNCTION paiements_fournisseurs_controler() RETURNS trigger AS $$
DECLARE
    genre_piece text;
    a_payer bigint;
    deja bigint;
BEGIN
    SELECT genre, a_payer_cents INTO genre_piece, a_payer
    FROM factures_recues WHERE organisation_id = NEW.organisation_id AND id = NEW.facture_recue_id;
    IF genre_piece IS DISTINCT FROM 'facture' THEN
        RAISE EXCEPTION 'un paiement fournisseur porte sur une facture reçue (pas sur une note de crédit)';
    END IF;
    SELECT COALESCE(sum(montant_cents), 0) INTO deja
    FROM paiements_fournisseurs WHERE facture_recue_id = NEW.facture_recue_id AND id <> NEW.id;
    IF deja + NEW.montant_cents > a_payer THEN
        RAISE EXCEPTION 'un paiement fournisseur ne dépasse pas le reste à payer';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER paiements_fournisseurs_controles BEFORE INSERT OR UPDATE ON paiements_fournisseurs
    FOR EACH ROW EXECUTE FUNCTION paiements_fournisseurs_controler();

-- L'état de paiement de chaque facture reçue : « a_payer », « partielle », « en_retard » (échue et pas soldée),
-- « payee », ou « note_credit » (rien à payer).
CREATE VIEW factures_recues_etats WITH (security_invoker = true) AS
SELECT fr.id AS facture_recue_id,
       p.paye_cents,
       CASE WHEN fr.genre = 'facture' THEN GREATEST(fr.a_payer_cents - p.paye_cents, 0) ELSE 0 END::bigint AS reste_cents,
       p.dernier_paiement,
       CASE
           WHEN fr.genre <> 'facture' THEN 'note_credit'
           WHEN fr.a_payer_cents - p.paye_cents <= 0 THEN 'payee'
           WHEN fr.date_echeance < j.aujourdhui THEN 'en_retard'
           WHEN p.paye_cents > 0 THEN 'partielle'
           ELSE 'a_payer'
       END AS etat,
       CASE WHEN fr.genre = 'facture' AND fr.a_payer_cents - p.paye_cents > 0 AND fr.date_echeance < j.aujourdhui
            THEN j.aujourdhui - fr.date_echeance ELSE 0 END AS jours_retard
FROM factures_recues fr
CROSS JOIN LATERAL (SELECT (now() AT TIME ZONE 'Europe/Brussels')::date AS aujourdhui) j
CROSS JOIN LATERAL (
    SELECT COALESCE(sum(montant_cents), 0)::bigint AS paye_cents, max(date_paiement) AS dernier_paiement
    FROM paiements_fournisseurs WHERE facture_recue_id = fr.id
) p;
