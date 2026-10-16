-- Avancements (étape 6-3). Un avancement dit, ligne par ligne, quelle part du devis de base et des avenants
-- acceptés a été réalisée pendant la période. Il deviendra une facture d'avancement à l'étape 7.

-- Brouillon (modifiable) puis validé (figé). « Facturé » arrivera avec les factures.
CREATE TYPE statut_avancement AS ENUM ('brouillon', 'valide');

CREATE TABLE avancements (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    chantier_id         bigint NOT NULL REFERENCES chantiers (id),
    -- Numéroté dans le chantier : 1, 2, 3…
    numero              integer NOT NULL CHECK (numero > 0),
    date_avancement     date NOT NULL DEFAULT current_date,
    reference           text NOT NULL DEFAULT '',
    statut              statut_avancement NOT NULL DEFAULT 'brouillon',
    -- Montant HT de la période (somme des lignes), recalculé par le serveur à chaque saisie.
    montant_ht_cents    bigint NOT NULL DEFAULT 0,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (chantier_id, numero)
);

-- Un seul brouillon à la fois par chantier : l'« avancement précédent » est alors toujours clair.
CREATE UNIQUE INDEX avancements_un_brouillon ON avancements (chantier_id) WHERE statut = 'brouillon';

-- Ce qui est avancé pendant la période, en centimes, pour une ligne chiffrée d'un devis ou d'un avenant,
-- ou pour l'ajustement (remise ou majoration) d'un devis ou d'un avenant. Une ligne à 0 n'est pas enregistrée.
CREATE TABLE avancement_lignes (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    avancement_id       bigint NOT NULL REFERENCES avancements (id) ON DELETE CASCADE,
    devis_ligne_id      bigint REFERENCES devis_lignes (id),
    ajustement_devis_id bigint REFERENCES devis (id),
    montant_cents       bigint NOT NULL CHECK (montant_cents <> 0),
    CHECK ((devis_ligne_id IS NULL) <> (ajustement_devis_id IS NULL))
);

CREATE UNIQUE INDEX avancement_lignes_ligne ON avancement_lignes (avancement_id, devis_ligne_id) WHERE devis_ligne_id IS NOT NULL;
CREATE UNIQUE INDEX avancement_lignes_ajustement ON avancement_lignes (avancement_id, ajustement_devis_id) WHERE ajustement_devis_id IS NOT NULL;
