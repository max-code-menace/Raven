-- Devis (en-tête seulement : les lignes arrivent avec l'éditeur d'encodage).
-- Montants en centimes. Les totaux sont recalculés par le serveur quand les lignes changent.

-- Statuts de Vertuoza : brouillon → à envoyer → envoi en cours → envoyé (ou échec) → signé / accepté / refusé.
CREATE TYPE statut_devis AS ENUM
    ('brouillon', 'a_envoyer', 'envoi_en_cours', 'envoye', 'echec_envoi', 'signe', 'accepte', 'refuse');

-- Compteurs de numérotation. On les incrémente dans la même transaction que la création du
-- document : la ligne est verrouillée, donc deux créations en même temps n'obtiennent jamais
-- le même numéro, et une création annulée ne consomme aucun numéro.
CREATE TABLE compteurs (
    nom     text PRIMARY KEY,
    valeur  bigint NOT NULL DEFAULT 0 CHECK (valeur >= 0)
);
INSERT INTO compteurs (nom) VALUES ('devis');

CREATE TABLE devis (
    id                      bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    numero                  bigint NOT NULL UNIQUE CHECK (numero > 0),
    date_devis              date NOT NULL DEFAULT current_date,
    reference               text NOT NULL DEFAULT '',
    statut                  statut_devis NOT NULL DEFAULT 'brouillon',
    -- Date du dernier changement de statut (affichée sous le statut dans la liste).
    statut_date             timestamptz NOT NULL DEFAULT now(),
    -- Le client est une entreprise ou une personne, jamais les deux. Vide tant que c'est un brouillon.
    client_entreprise_id    bigint REFERENCES entreprises (id),
    client_contact_id       bigint REFERENCES contacts (id),
    montant_ht_cents        bigint NOT NULL DEFAULT 0,
    montant_ttc_cents       bigint NOT NULL DEFAULT 0,
    created_at              timestamptz NOT NULL DEFAULT now(),
    updated_at              timestamptz NOT NULL DEFAULT now(),
    CHECK (client_entreprise_id IS NULL OR client_contact_id IS NULL)
);

CREATE INDEX devis_date ON devis (date_devis DESC, id DESC);
CREATE INDEX devis_statut ON devis (statut);
CREATE INDEX devis_client_entreprise ON devis (client_entreprise_id);
CREATE INDEX devis_client_contact ON devis (client_contact_id);
