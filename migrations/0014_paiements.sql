-- Paiements des factures (étape 8-1).
-- Un paiement est enregistré sur une facture numérotée (pas sur un brouillon ni sur une note de crédit).
-- Le statut de paiement n'est pas enregistré : il se calcule à chaque lecture depuis les paiements, les notes de
-- crédit et l'échéance (« Payée », « Partiellement payée », « En retard »). Montants en centimes.

-- Les méthodes de paiement de Vertuoza (Réglages › Propriétés).
CREATE TABLE methodes_paiement (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    libelle     text NOT NULL UNIQUE,
    position    integer NOT NULL,
    actif       boolean NOT NULL DEFAULT true
);

INSERT INTO methodes_paiement (libelle, position) VALUES
    ('Virement', 1), ('Bancontact', 2), ('Carte de crédit', 3), ('Chèque', 4), ('Espèces', 5), ('Bancaire', 6);

CREATE TABLE paiements (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    facture_id      bigint NOT NULL REFERENCES factures (id),
    date_paiement   date NOT NULL,
    montant_cents   bigint NOT NULL CHECK (montant_cents > 0),
    methode_id      bigint NOT NULL REFERENCES methodes_paiement (id),
    remarque        text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX paiements_facture ON paiements (facture_id, date_paiement);

-- Garde-fou dans la base : pas de paiement sur un brouillon ni sur une note de crédit.
CREATE FUNCTION paiements_verifier() RETURNS trigger AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM factures WHERE id = NEW.facture_id AND numero IS NOT NULL AND genre <> 'note_credit') THEN
        RAISE EXCEPTION 'un paiement se rattache à une facture validée';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER paiements_sur_facture_validee BEFORE INSERT OR UPDATE ON paiements
    FOR EACH ROW EXECUTE FUNCTION paiements_verifier();

-- L'état de chaque facture, calculé à la lecture (la date du jour change tous les jours) :
-- payé, crédité (notes de crédit validées), reste à recevoir, et l'état affiché :
--   brouillon, a_envoyer, envoyee (aucun paiement, pas encore échue),
--   partielle (une partie payée), en_retard (échue et pas soldée), payee, creditee (soldée par notes de crédit).
-- Une note de crédit garde son statut d'envoi.
CREATE VIEW factures_etats AS
SELECT f.id AS facture_id,
       s.paye_cents,
       s.credite_cents,
       s.reste_cents,
       s.dernier_paiement,
       CASE
           WHEN f.numero IS NULL OR f.genre = 'note_credit' THEN f.statut::text
           WHEN s.reste_cents <= 0 AND s.paye_cents > 0 THEN 'payee'
           WHEN s.reste_cents <= 0 THEN 'creditee'
           WHEN f.date_echeance < j.aujourdhui THEN 'en_retard'
           WHEN s.paye_cents > 0 THEN 'partielle'
           ELSE f.statut::text
       END AS etat,
       CASE WHEN f.numero IS NOT NULL AND f.genre <> 'note_credit' AND s.reste_cents > 0 AND f.date_echeance < j.aujourdhui
            THEN j.aujourdhui - f.date_echeance ELSE 0 END AS jours_retard
FROM factures f
CROSS JOIN LATERAL (SELECT (now() AT TIME ZONE 'Europe/Brussels')::date AS aujourdhui) j
CROSS JOIN LATERAL (
    SELECT p.paye_cents, n.credite_cents, p.dernier_paiement,
           CASE WHEN f.numero IS NULL OR f.genre = 'note_credit' THEN 0
                ELSE f.montant_ttc_cents - p.paye_cents - n.credite_cents END AS reste_cents
    FROM (SELECT COALESCE(sum(montant_cents), 0)::bigint AS paye_cents, max(date_paiement) AS dernier_paiement
          FROM paiements WHERE facture_id = f.id) p,
         (SELECT COALESCE(sum(montant_ttc_cents), 0)::bigint AS credite_cents
          FROM factures WHERE facture_origine_id = f.id AND numero IS NOT NULL) n
) s;

-- Le journal d'une facture : paiements, e-mails, rappels, consultation par le client…
CREATE TABLE facture_evenements (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    facture_id  bigint NOT NULL REFERENCES factures (id) ON DELETE CASCADE,
    genre       text NOT NULL CHECK (genre IN ('validation', 'envoi', 'paiement', 'paiement_supprime', 'email', 'rappel', 'client')),
    detail      text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX facture_evenements_facture ON facture_evenements (facture_id, created_at);
