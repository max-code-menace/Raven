-- Avenants (étape 6-2). Un avenant est un devis rattaché à un chantier : il réutilise les lignes, les totaux,
-- le PDF et les statuts du devis. Il corrige le devis de base, en plus ou en moins, après l'acceptation.

ALTER TABLE devis
    -- Rempli seulement pour un avenant : le chantier qu'il corrige.
    ADD COLUMN chantier_id      bigint REFERENCES chantiers (id),
    -- Jours ouvrables supplémentaires accordés par l'avenant.
    ADD COLUMN jours_ouvrables  integer NOT NULL DEFAULT 0 CHECK (jours_ouvrables BETWEEN 0 AND 999);

-- Un devis a un numéro unique dans la numérotation des devis ; un avenant a un numéro propre à son chantier
-- (le devis de base compte pour le n° 1, le premier avenant est le n° 2, comme dans Vertuoza).
ALTER TABLE devis DROP CONSTRAINT devis_numero_key;
CREATE UNIQUE INDEX devis_numero_unique ON devis (numero) WHERE chantier_id IS NULL;
CREATE UNIQUE INDEX devis_avenant_numero_unique ON devis (chantier_id, numero) WHERE chantier_id IS NOT NULL;
CREATE INDEX devis_chantier ON devis (chantier_id) WHERE chantier_id IS NOT NULL;
