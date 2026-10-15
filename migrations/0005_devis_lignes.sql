-- Lignes de devis (étape 5b-2 : titres, sous-titres, textes et postes libres).
-- Les autres types (ouvrage, fourniture, main d'œuvre…) s'ajouteront à l'étape 5b-4.

CREATE TYPE genre_ligne AS ENUM ('titre', 'sous_titre', 'texte', 'poste_libre');

CREATE TABLE devis_lignes (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    devis_id            bigint NOT NULL REFERENCES devis (id) ON DELETE CASCADE,
    -- Ordre d'affichage. Il peut y avoir des trous (après une suppression), jamais de doublon.
    position            integer NOT NULL,
    genre               genre_ligne NOT NULL,
    description         text NOT NULL DEFAULT '',
    -- Quantité et prix unitaire en 1/100 000 (5 décimales, comme Vertuoza) :
    -- 2,5 → 250000 ; 19,99 € → 1999000. Vides (NULL) tant qu'ils ne sont pas encodés.
    quantite_e5         bigint,
    unite               text NOT NULL DEFAULT '',
    prix_unitaire_e5    bigint,
    taux_tva_id         bigint REFERENCES taux_tva (id),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    -- Deux lignes peuvent échanger leur place dans la même requête.
    UNIQUE (devis_id, position) DEFERRABLE INITIALLY DEFERRED,
    -- Seul un poste libre porte une quantité, un prix et une TVA.
    CHECK (genre = 'poste_libre' OR (quantite_e5 IS NULL AND prix_unitaire_e5 IS NULL AND taux_tva_id IS NULL AND unite = ''))
);

CREATE INDEX devis_lignes_devis ON devis_lignes (devis_id, position);
