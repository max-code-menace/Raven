-- Chantiers (étape 6-1). Un chantier naît d'un devis accepté, dans la même transaction que l'acceptation.
-- La référence, le client, l'adresse et le montant restent ceux du devis (un devis accepté ne se modifie plus).

CREATE TYPE statut_chantier AS ENUM ('en_cours', 'termine');

-- Numérotation simple et continue : 1, 2, 3…
INSERT INTO compteurs (nom) VALUES ('chantier');

CREATE TABLE chantiers (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    numero      bigint NOT NULL UNIQUE CHECK (numero > 0),
    -- Un devis donne au plus un chantier.
    devis_id    bigint NOT NULL UNIQUE REFERENCES devis (id),
    statut      statut_chantier NOT NULL DEFAULT 'en_cours',
    -- Début de chantier : la date d'acceptation par défaut, modifiable ensuite.
    date_debut  date NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX chantiers_statut ON chantiers (statut, numero DESC);

-- Les devis déjà acceptés reçoivent leur chantier, dans l'ordre des acceptations.
INSERT INTO chantiers (numero, devis_id, date_debut)
SELECT row_number() OVER (ORDER BY COALESCE(date_acceptation, date_devis), numero),
       id,
       COALESCE(date_acceptation, date_devis)
FROM devis
WHERE statut = 'accepte';

UPDATE compteurs SET valeur = (SELECT count(*) FROM chantiers) WHERE nom = 'chantier';

-- Le journal du devis note la création de son chantier.
ALTER TABLE devis_evenements DROP CONSTRAINT devis_evenements_genre_check;
ALTER TABLE devis_evenements ADD CONSTRAINT devis_evenements_genre_check
    CHECK (genre IN ('creation', 'statut', 'pdf', 'duplication', 'chantier'));
