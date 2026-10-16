-- Suivi de chantier complet (étape 9-2) : les achats réels d'un chantier (factures fournisseurs, sous-traitants,
-- location…), saisis à la main en attendant la réception des factures fournisseurs par Peppol (phase 2).
-- Ils donnent le budget d'achat dépensé / disponible et le bénéfice actuel. Montants HT en centimes.

CREATE TYPE categorie_achat AS ENUM ('fourniture', 'sous_traitant', 'equipement', 'main_oeuvre', 'autre');

CREATE TABLE chantier_achats (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    chantier_id     bigint NOT NULL REFERENCES chantiers (id) ON DELETE CASCADE,
    date_achat      date NOT NULL,
    categorie       categorie_achat NOT NULL,
    fournisseur     text NOT NULL DEFAULT '',
    description     text NOT NULL DEFAULT '',
    montant_ht_cents bigint NOT NULL CHECK (montant_ht_cents <> 0),
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX chantier_achats_chantier ON chantier_achats (chantier_id, date_achat);
