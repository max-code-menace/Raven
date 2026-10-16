-- Rappels de paiement (étape 8-3) et réglages généraux (une seule ligne).
-- Semi-automatique : l'application propose les factures à relancer, on envoie d'un clic.
-- Les rappels envoyés sont les e-mails des modèles « rappel_doux » et « rappel_ferme » (table `emails`).

CREATE TABLE reglages (
    id                      smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    -- Premier rappel proposé ce nombre de jours après l'échéance.
    rappel_premier_jours    integer NOT NULL DEFAULT 7 CHECK (rappel_premier_jours BETWEEN 1 AND 365),
    -- Rappel suivant proposé ce nombre de jours après le précédent.
    rappel_suivant_jours    integer NOT NULL DEFAULT 14 CHECK (rappel_suivant_jours BETWEEN 1 AND 365),
    -- L'adresse du comptable pour l'export des factures (étape 8-4).
    email_comptable         text NOT NULL DEFAULT '',
    updated_at              timestamptz NOT NULL DEFAULT now()
);

INSERT INTO reglages DEFAULT VALUES;

CREATE INDEX emails_rappels ON emails (facture_id, created_at) WHERE modele IN ('rappel_doux', 'rappel_ferme');
