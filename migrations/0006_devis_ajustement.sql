-- Devis : un ajustement (remise ou majoration), étape 5b-3.
-- La valeur est signée : négative = remise, positive = majoration.
--   mode « pourcentage » : valeur en centièmes de pour cent (−500 = remise de 5 %)
--   mode « montant »     : valeur en centimes (−10000 = remise de 100 €)
-- Le serveur répartit l'ajustement entre les taux de TVA au prorata des bases.

ALTER TABLE devis
    ADD COLUMN ajustement_libelle   text NOT NULL DEFAULT '',
    ADD COLUMN ajustement_mode      text NOT NULL DEFAULT 'aucun'
        CHECK (ajustement_mode IN ('aucun', 'pourcentage', 'montant')),
    ADD COLUMN ajustement_valeur    bigint NOT NULL DEFAULT 0,
    ADD CONSTRAINT devis_ajustement_coherent CHECK (ajustement_mode <> 'aucun' OR ajustement_valeur = 0);
