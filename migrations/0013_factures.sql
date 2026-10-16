-- Factures et notes de crédit (étape 7).
-- Une facture naît en brouillon, sans numéro : on peut la modifier ou la supprimer. Quand on la valide, elle
-- reçoit le numéro suivant (dans la même transaction que le compteur : pas de trou, pas de doublon), sa date
-- d'échéance et sa communication structurée ; elle ne se modifie plus et ne se supprime jamais. Pour la corriger,
-- on crée une note de crédit. Montants en centimes.

-- simple : sans devis ; acompte, avancement, finale : liées à un chantier ; note_credit : corrige une facture.
CREATE TYPE genre_facture AS ENUM ('simple', 'acompte', 'avancement', 'finale', 'note_credit');

-- Les paiements (partiellement payé, payé) arriveront avec l'étape 8.
CREATE TYPE statut_facture AS ENUM ('brouillon', 'a_envoyer', 'envoyee');

-- La numérotation continue celle de Vertuoza : dernière facture n° 124, dernière note de crédit n° 5.
-- Les notes de crédit ont leur propre numérotation (comme chez Ubique dans Vertuoza).
INSERT INTO compteurs (nom, valeur) VALUES ('facture', 124), ('note_credit', 5);

CREATE TABLE factures (
    id                      bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    genre                   genre_facture NOT NULL,
    -- Vide tant que la facture est un brouillon.
    numero                  bigint CHECK (numero > 0),
    statut                  statut_facture NOT NULL DEFAULT 'brouillon',
    date_facture            date NOT NULL DEFAULT current_date,
    -- Calculée à la validation, d'après les conditions de paiement.
    date_echeance           date,
    reference               text NOT NULL DEFAULT '',
    responsable             text NOT NULL DEFAULT '',
    -- Le client facturé : une entreprise ou une personne, jamais les deux.
    client_entreprise_id    bigint REFERENCES entreprises (id),
    client_contact_id       bigint REFERENCES contacts (id),
    conditions_paiement_id  bigint REFERENCES conditions_paiement (id),
    -- Taux proposé pour les nouvelles lignes d'une facture simple.
    taux_tva_id             bigint REFERENCES taux_tva (id),
    remarque                text NOT NULL DEFAULT '',
    chantier_id             bigint REFERENCES chantiers (id),
    -- La facture d'avancement (ou finale) reprend un avancement validé : une seule facture par avancement.
    avancement_id           bigint UNIQUE REFERENCES avancements (id),
    -- La facture corrigée par une note de crédit.
    facture_origine_id      bigint REFERENCES factures (id),
    -- Communication structurée belge (+++123/4567/89012+++), calculée à la validation.
    vcs                     text,
    montant_ht_cents        bigint NOT NULL DEFAULT 0,
    montant_tva_cents       bigint NOT NULL DEFAULT 0,
    montant_ttc_cents       bigint NOT NULL DEFAULT 0,
    validee_le              timestamptz,
    envoyee_le              timestamptz,
    created_at              timestamptz NOT NULL DEFAULT now(),
    updated_at              timestamptz NOT NULL DEFAULT now(),
    CHECK ((statut = 'brouillon') = (numero IS NULL)),
    CHECK (client_entreprise_id IS NULL OR client_contact_id IS NULL),
    CHECK ((genre = 'note_credit') = (facture_origine_id IS NOT NULL)),
    CHECK (genre NOT IN ('acompte', 'avancement', 'finale') OR chantier_id IS NOT NULL),
    CHECK (genre NOT IN ('avancement', 'finale') OR avancement_id IS NOT NULL),
    CHECK (genre IN ('avancement', 'finale') OR avancement_id IS NULL)
);

-- Deux numérotations : les factures (tous types) et les notes de crédit.
CREATE UNIQUE INDEX factures_numero ON factures (numero) WHERE genre <> 'note_credit' AND numero IS NOT NULL;
CREATE UNIQUE INDEX notes_credit_numero ON factures (numero) WHERE genre = 'note_credit' AND numero IS NOT NULL;
CREATE INDEX factures_date ON factures (date_facture DESC, id DESC);
CREATE INDEX factures_chantier ON factures (chantier_id) WHERE chantier_id IS NOT NULL;
CREATE INDEX factures_origine ON factures (facture_origine_id) WHERE facture_origine_id IS NOT NULL;

-- Les lignes d'une facture sont une copie figée : changer un devis ou un taux de TVA plus tard ne change rien
-- à une facture déjà faite. `total_cents` est enregistré (une ligne d'avancement vaut le montant avancé,
-- pas forcément quantité × prix).
CREATE TABLE facture_lignes (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    facture_id          bigint NOT NULL REFERENCES factures (id) ON DELETE CASCADE,
    position            integer NOT NULL,
    genre               text NOT NULL CHECK (genre IN ('titre', 'texte', 'poste')),
    description         text NOT NULL DEFAULT '',
    quantite_e5         bigint,
    unite               text NOT NULL DEFAULT '',
    prix_unitaire_e5    bigint,
    taux_tva_id         bigint REFERENCES taux_tva (id),
    -- Copie du taux au moment de la ligne (600 = 6 %).
    taux_centiemes      integer CHECK (taux_centiemes BETWEEN 0 AND 10000),
    total_cents         bigint NOT NULL DEFAULT 0,
    UNIQUE (facture_id, position) DEFERRABLE INITIALLY DEFERRED,
    CHECK (genre = 'poste' OR (quantite_e5 IS NULL AND prix_unitaire_e5 IS NULL AND taux_tva_id IS NULL
                               AND taux_centiemes IS NULL AND total_cents = 0 AND unite = '')),
    CHECK ((taux_tva_id IS NULL) = (taux_centiemes IS NULL))
);

CREATE INDEX facture_lignes_facture ON facture_lignes (facture_id, position);

-- Garde-fou dans la base elle-même : une facture numérotée (comptabilisée) ne se supprime jamais,
-- et ses lignes ne changent plus.
CREATE FUNCTION factures_proteger() RETURNS trigger AS $$
BEGIN
    IF TG_TABLE_NAME = 'factures' THEN
        IF OLD.numero IS NOT NULL THEN
            RAISE EXCEPTION 'une facture numérotée ne se supprime pas';
        END IF;
        RETURN OLD;
    END IF;
    IF EXISTS (SELECT 1 FROM factures WHERE id = COALESCE(NEW.facture_id, OLD.facture_id) AND numero IS NOT NULL) THEN
        RAISE EXCEPTION 'les lignes d''une facture numérotée ne changent plus';
    END IF;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER factures_pas_de_suppression BEFORE DELETE ON factures
    FOR EACH ROW EXECUTE FUNCTION factures_proteger();
CREATE TRIGGER facture_lignes_figees BEFORE INSERT OR UPDATE OR DELETE ON facture_lignes
    FOR EACH ROW EXECUTE FUNCTION factures_proteger();
