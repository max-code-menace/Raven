-- Import d'un devis ou d'un avenant depuis le modèle Excel (après l'étape 9).
-- Le fichier est lu une fois, ses lignes sont gardées ici le temps de l'aperçu ; « Importer » les copie dans le devis.
-- Le fichier lui-même n'est pas gardé.

CREATE TABLE imports_excel (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    devis_id            bigint NOT NULL REFERENCES devis (id) ON DELETE CASCADE,
    nom_fichier         text NOT NULL DEFAULT '',
    -- Les lignes lues, en JSON (type, description, quantité, unité, prix, ligne du fichier).
    lignes              text NOT NULL,
    -- Le « TOTAL H.T.V.A. » écrit dans le fichier, pour comparer.
    total_fichier_cents bigint,
    -- Le taux de TVA écrit dans le fichier (« T.V.A. 6% » → 600).
    taux_fichier        integer,
    alertes             text NOT NULL DEFAULT '',
    created_at          timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX imports_excel_devis ON imports_excel (devis_id);

-- Le journal du devis note les imports.
ALTER TABLE devis_evenements DROP CONSTRAINT devis_evenements_genre_check;
ALTER TABLE devis_evenements ADD CONSTRAINT devis_evenements_genre_check
    CHECK (genre IN ('creation', 'statut', 'pdf', 'duplication', 'chantier', 'email', 'client', 'import'));
