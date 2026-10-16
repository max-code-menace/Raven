-- Bibliothèque de prix (étape 9-3), comme celle de Vertuoza : des ouvrages (description, type, catégorie,
-- fournisseur, unité, prix d'achat, marge, prix de vente, remarque) qu'on insère dans un devis.
-- Insérer un ouvrage copie ses valeurs dans une ligne du devis : changer la bibliothèque ensuite ne change
-- aucun devis. Prix en 1/100 000 d'euro, comme les lignes de devis.

CREATE TABLE ouvrages (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    description     text NOT NULL CHECK (description <> ''),
    genre           genre_ligne NOT NULL
        CHECK (genre::text IN ('poste_libre', 'ouvrage', 'fourniture', 'equipement', 'main_oeuvre', 'sous_traitant')),
    categorie       text NOT NULL DEFAULT '',
    fournisseur     text NOT NULL DEFAULT '',
    unite           text NOT NULL DEFAULT '',
    prix_achat_e5   bigint CHECK (prix_achat_e5 >= 0),
    prix_vente_e5   bigint NOT NULL,
    remarque        text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX ouvrages_description ON ouvrages (lower(description));
CREATE INDEX ouvrages_categorie ON ouvrages (categorie);
