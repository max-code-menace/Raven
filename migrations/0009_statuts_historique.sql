-- Accepter / refuser un devis (étape 5c-2) et historique des événements (étape 5c-3).

ALTER TABLE devis
    -- Dates de la décision du client, saisies au moment d'accepter ou de refuser.
    ADD COLUMN date_acceptation     date,
    ADD COLUMN date_refus           date,
    ADD COLUMN motif_refus          text NOT NULL DEFAULT '';

-- Le journal d'un devis : ce qui s'est passé, et quand. Une ligne par événement important
-- (création, changement de statut, téléchargement du PDF, duplication), jamais modifiée.
CREATE TABLE devis_evenements (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    devis_id    bigint NOT NULL REFERENCES devis (id) ON DELETE CASCADE,
    genre       text NOT NULL CHECK (genre IN ('creation', 'statut', 'pdf', 'duplication')),
    detail      text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX devis_evenements_devis ON devis_evenements (devis_id, created_at DESC, id DESC);

-- Les devis qui existaient déjà gardent au moins la trace de leur création.
INSERT INTO devis_evenements (devis_id, genre, detail, created_at)
SELECT id, 'creation', 'Devis créé', created_at FROM devis;
