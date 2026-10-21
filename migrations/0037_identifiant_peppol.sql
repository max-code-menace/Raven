-- Étape 5-1 (envoi Peppol) : l'identifiant Peppol d'un client, quand il ne se déduit pas de son numéro de TVA.
-- Un client belge n'en a pas besoin : son adresse Peppol est son numéro d'entreprise (« 0208:0123456789 »), tiré du
-- numéro de TVA. Ce champ sert pour un client étranger ou inscrit sous un autre identifiant (GLN « 0088:… », etc.).
-- Format de la norme ISO 6523 : le schéma sur 4 chiffres, deux-points, le numéro.
ALTER TABLE entreprises ADD COLUMN peppol_id text CHECK (peppol_id ~ '^[0-9]{4}:[A-Za-z0-9._-]{1,50}$');
ALTER TABLE contacts ADD COLUMN peppol_id text CHECK (peppol_id ~ '^[0-9]{4}:[A-Za-z0-9._-]{1,50}$');
