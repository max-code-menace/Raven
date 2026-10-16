-- Page client en ligne (étape 9-1) : le client ouvre le lien secret (colonne `jeton`, migration 0015),
-- consulte la pièce, signe un devis, paie une facture par QR code.
-- La signature en ligne garde le nom tapé par le client et l'heure ; le devis passe à « Signé »,
-- puis l'entreprise l'accepte (comme dans Vertuoza).

ALTER TABLE devis ADD COLUMN signe_le timestamptz;
ALTER TABLE devis ADD COLUMN signe_par text NOT NULL DEFAULT '';
-- Première consultation par le client (affichée sur la pièce).
ALTER TABLE devis ADD COLUMN consulte_le timestamptz;
ALTER TABLE factures ADD COLUMN consulte_le timestamptz;
