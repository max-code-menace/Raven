-- Prix d'achat à la ligne, remarque par ligne, et l'option « marge et prix d'achat » du devis (étape 5b-4).

ALTER TABLE devis_lignes
    -- Prix d'achat unitaire, en 1/100 000 comme le prix de vente. Vide tant qu'il n'est pas encodé.
    ADD COLUMN prix_achat_unitaire_e5   bigint,
    -- Texte libre affiché sous la ligne.
    ADD COLUMN remarque                 text NOT NULL DEFAULT '';

-- Les types chiffrés portent quantité, unité, prix et TVA ; les autres (titre, sous-titre, texte,
-- saut de page) n'en portent pas.
ALTER TABLE devis_lignes DROP CONSTRAINT devis_lignes_check;
ALTER TABLE devis_lignes ADD CONSTRAINT devis_lignes_champs_chiffres CHECK (
    genre IN ('poste_libre', 'ouvrage', 'fourniture', 'equipement', 'main_oeuvre', 'sous_traitant')
    OR (quantite_e5 IS NULL AND prix_unitaire_e5 IS NULL AND prix_achat_unitaire_e5 IS NULL
        AND taux_tva_id IS NULL AND unite = '')
);

ALTER TABLE devis
    -- Interrupteur de l'éditeur : affiche les prix d'achat, la marge et la rentabilité.
    -- Désactivé, les prix d'achat déjà encodés sont gardés.
    ADD COLUMN marge_activee                boolean NOT NULL DEFAULT false,
    -- Frais généraux en pour cent des achats, en centièmes (1000 = 10 %).
    ADD COLUMN frais_generaux_centiemes     integer NOT NULL DEFAULT 0
        CHECK (frais_generaux_centiemes BETWEEN 0 AND 10000);
