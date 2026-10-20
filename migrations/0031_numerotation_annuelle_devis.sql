-- Numérotation annuelle des devis : le numéro repart à 1 chaque année civile, comme les factures (migration 0022) et
-- comme dans Vertuoza (devis n° 2026-57). Le numéro s'imprime avec l'année : « 2026-57 ».
-- `compteurs.annee` : l'année du dernier devis numéroté ; un devis créé une année suivante repart de 1.
-- Le compteur actuel est celui de l'année en cours (2026 pour Ubique, migrée en 2026). Un compteur encore à 0 garde
-- une année vide : son premier devis prendra le n° 1 de son année.
-- Les compteurs sont protégés par la sécurité au niveau des lignes (migration 0026) : sans organisation réglée, la
-- mise à jour ne verrait aucune ligne. On lève `FORCE` le temps de la mise à jour, dans la même transaction.
ALTER TABLE compteurs NO FORCE ROW LEVEL SECURITY;
UPDATE compteurs SET annee = EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int WHERE nom = 'devis' AND valeur > 0;
ALTER TABLE compteurs FORCE ROW LEVEL SECURITY;

-- Un numéro de devis est unique dans son organisation et son année (un avenant a son propre numéro dans son chantier).
DROP INDEX devis_numero_unique;
CREATE UNIQUE INDEX devis_numero_unique ON devis (organisation_id, (EXTRACT(YEAR FROM date_devis)), numero) WHERE chantier_id IS NULL;
