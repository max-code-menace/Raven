-- Export des factures pour le comptable (étape 8-4), comme « Export factures » de Vertuoza :
-- un dossier par période, au format PDF, UBL 2.1 ou les deux, avec toutes les pièces ou seulement les non exportées.
-- Le dossier est un fichier ZIP gardé ici (téléchargeable et envoyable par e-mail au comptable).

CREATE TABLE exports (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    date_debut      date NOT NULL,
    date_fin        date NOT NULL CHECK (date_fin >= date_debut),
    format          text NOT NULL CHECK (format IN ('pdf', 'ubl', 'les_deux')),
    contenu         text NOT NULL CHECK (contenu IN ('toutes', 'non_exportees')),
    nombre_pieces   integer NOT NULL,
    nom_fichier     text NOT NULL,
    fichier         bytea NOT NULL,
    envoye_le       timestamptz,
    envoye_a        text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now()
);

-- Les pièces (factures et notes de crédit) de chaque export : sert au choix « uniquement non exportées ».
CREATE TABLE export_pieces (
    export_id   bigint NOT NULL REFERENCES exports (id) ON DELETE CASCADE,
    facture_id  bigint NOT NULL REFERENCES factures (id),
    PRIMARY KEY (export_id, facture_id)
);

CREATE INDEX export_pieces_facture ON export_pieces (facture_id);

ALTER TABLE emails ADD COLUMN export_id bigint REFERENCES exports (id) ON DELETE SET NULL;
