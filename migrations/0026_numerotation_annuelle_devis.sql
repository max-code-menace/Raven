-- Numérotation annuelle des devis : le numéro repart à 1 chaque année civile, comme les factures (migration 0022) et
-- comme dans Vertuoza (devis n° 2026-57). Le numéro s'imprime avec l'année : « 2026-57 ».
-- `compteurs.annee` : l'année du dernier devis numéroté ; un devis créé une année suivante repart de 1.
-- Le compteur actuel (repris de Vertuoza) est celui de l'année en cours : 2026 pour la vraie base, migrée en 2026.
UPDATE compteurs SET annee = EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int WHERE nom = 'devis';

-- Un numéro de devis est unique dans son année (un avenant a son propre numéro dans son chantier).
DROP INDEX devis_numero_unique;
CREATE UNIQUE INDEX devis_numero_unique ON devis ((EXTRACT(YEAR FROM date_devis)), numero) WHERE chantier_id IS NULL;
