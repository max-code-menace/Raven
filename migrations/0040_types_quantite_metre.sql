-- Refonte du design, étape 4-2 : type de quantité de chaque ligne chiffrée et métré dans la ligne.
-- Type de quantité (fiche QuoteTree du design de Raven) : QF « Quantité fixe » (par défaut, aussi pour les lignes
-- existantes), QP « Quantité estimée » (facturée selon le métré réel), Fft « Forfait ». Il ne change aucun calcul.
CREATE TYPE type_quantite AS ENUM ('fixe', 'estimee', 'forfait');
ALTER TABLE devis_lignes ADD COLUMN type_quantite type_quantite NOT NULL DEFAULT 'fixe';

-- Métré : les mesures d'une ligne (longueur × largeur, ou une seule longueur ; une déduction compte en négatif).
-- La somme des mesures est recopiée dans `devis_lignes.quantite_e5` à chaque changement : totaux, avancements, PDF
-- et factures lisent la quantité comme avant.
CREATE TABLE devis_metres (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organisation_id     bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    ligne_id            bigint NOT NULL,
    position            integer NOT NULL,
    libelle             text NOT NULL DEFAULT '' CHECK (length(libelle) <= 200),
    -- En 1/100 000, comme les quantités.
    longueur_e5         bigint NOT NULL DEFAULT 0 CHECK (longueur_e5 >= 0),
    -- NULL : mesure à une seule dimension (longueur seule).
    largeur_e5          bigint CHECK (largeur_e5 >= 0),
    deduction           boolean NOT NULL DEFAULT false,
    created_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organisation_id, id),
    FOREIGN KEY (organisation_id, ligne_id) REFERENCES devis_lignes (organisation_id, id) ON DELETE CASCADE
);
CREATE INDEX devis_metres_ligne ON devis_metres (organisation_id, ligne_id, position);

ALTER TABLE devis_metres ENABLE ROW LEVEL SECURITY;
ALTER TABLE devis_metres FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON devis_metres USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());
