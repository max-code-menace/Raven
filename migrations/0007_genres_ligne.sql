-- Nouveaux types de ligne (étape 5b-4). Une valeur d'enum ajoutée ne peut pas être utilisée dans
-- la même transaction : la suite (contrainte, colonnes) est dans 0008.
ALTER TYPE genre_ligne ADD VALUE IF NOT EXISTS 'ouvrage';
ALTER TYPE genre_ligne ADD VALUE IF NOT EXISTS 'fourniture';
ALTER TYPE genre_ligne ADD VALUE IF NOT EXISTS 'equipement';
ALTER TYPE genre_ligne ADD VALUE IF NOT EXISTS 'main_oeuvre';
ALTER TYPE genre_ligne ADD VALUE IF NOT EXISTS 'sous_traitant';
ALTER TYPE genre_ligne ADD VALUE IF NOT EXISTS 'saut_de_page';
