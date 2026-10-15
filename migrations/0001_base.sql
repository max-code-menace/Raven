-- Tables de base d'Ubifacture : paramètres société, taux de TVA, conditions de paiement,
-- entreprises et contacts. Montants en centimes, dates en timestamptz.

-- ─── Paramètres société (une seule ligne) ───────────────────────────────────
CREATE TABLE societe (
    id                      smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    nom                     text NOT NULL,
    nom_abrege              text NOT NULL,
    rue                     text NOT NULL DEFAULT '',
    numero                  text NOT NULL DEFAULT '',
    code_postal             text NOT NULL DEFAULT '',
    ville                   text NOT NULL DEFAULT '',
    pays                    text NOT NULL DEFAULT 'BE',
    email                   text NOT NULL,
    telephone               text NOT NULL,
    site_web                text,
    numero_tva              text,
    devise                  text NOT NULL DEFAULT 'EUR',
    -- Chemin du PDF des conditions générales, joint aux devis et aux factures.
    conditions_generales    text,
    created_at              timestamptz NOT NULL DEFAULT now(),
    updated_at              timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE societe_comptes_bancaires (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    iban        text NOT NULL,
    bic         text,
    par_defaut  boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- Un seul compte par défaut à la fois.
CREATE UNIQUE INDEX societe_un_seul_compte_par_defaut
    ON societe_comptes_bancaires (par_defaut) WHERE par_defaut;

-- ─── Taux de TVA ────────────────────────────────────────────────────────────
-- taux_centiemes : 600 = 6,00 %. libelle_legal = mention affichée sur la facture.
CREATE TABLE taux_tva (
    id                bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    libelle           text NOT NULL,
    taux_centiemes    integer NOT NULL CHECK (taux_centiemes BETWEEN 0 AND 10000),
    autoliquidation   boolean NOT NULL DEFAULT false,
    actif             boolean NOT NULL DEFAULT true,
    created_at        timestamptz NOT NULL DEFAULT now()
);

INSERT INTO taux_tva (libelle, taux_centiemes, autoliquidation) VALUES
    ('TVA acquittée par autoliquidation', 0,    true),
    ('Taux réduit',                       600,  false),
    ('Taux normal',                       2100, false);

-- ─── Conditions de paiement ────────────────────────────────────────────────
CREATE TYPE type_condition_paiement AS ENUM ('apres_facturation', 'fin_de_mois', 'comptant');

CREATE TABLE conditions_paiement (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    type        type_condition_paiement NOT NULL,
    jours       integer NOT NULL CHECK (jours >= 0),
    CHECK ((type = 'comptant') = (jours = 0)),
    UNIQUE (type, jours)
);

INSERT INTO conditions_paiement (type, jours) VALUES
    ('apres_facturation', 3),
    ('apres_facturation', 7),
    ('apres_facturation', 10),
    ('apres_facturation', 15),
    ('apres_facturation', 30),
    ('apres_facturation', 60),
    ('fin_de_mois', 10),
    ('fin_de_mois', 15),
    ('fin_de_mois', 30),
    ('comptant', 0);

-- ─── Entreprises ───────────────────────────────────────────────────────────
-- Profils : prospect, client, fournisseur_materiaux, fournisseur_sous_traitant,
-- co_traitant, architecte, autre, fournisseur_autres, ex_client, proprietaire.
-- Langue : fr, en, nl_be, nl.
CREATE TABLE entreprises (
    id                          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    nom                         text NOT NULL,
    profils                     text[] NOT NULL DEFAULT '{}'
        CHECK (profils <@ ARRAY['prospect','client','fournisseur_materiaux','fournisseur_sous_traitant',
                                'co_traitant','architecte','autre','fournisseur_autres','ex_client','proprietaire']),
    numero_tva                  text,
    identifiant_comptable       text,
    franco_cents                bigint CHECK (franco_cents >= 0),
    rue                         text NOT NULL DEFAULT '',
    numero                      text NOT NULL DEFAULT '',
    code_postal                 text NOT NULL DEFAULT '',
    ville                       text NOT NULL DEFAULT '',
    pays                        text NOT NULL DEFAULT 'BE',
    facturation_identique       boolean NOT NULL DEFAULT true,
    facturation_rue             text,
    facturation_numero          text,
    facturation_code_postal     text,
    facturation_ville           text,
    facturation_pays            text,
    email                       text,
    email_bis                   text,
    telephone                   text,
    site_web                    text,
    iban                        text,
    bic                         text,
    conditions_paiement_id      bigint REFERENCES conditions_paiement (id),
    taux_tva_id                 bigint REFERENCES taux_tva (id),
    source                      text CHECK (source IN ('apporteur_affaires','client','marketing','prospection',
                                                       'recommandation_client','reseaux_sociaux','site_web')),
    langue                      text NOT NULL DEFAULT 'fr' CHECK (langue IN ('fr','en','nl_be','nl')),
    description                 text,
    created_at                  timestamptz NOT NULL DEFAULT now(),
    updated_at                  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX entreprises_nom ON entreprises (lower(nom));
CREATE INDEX entreprises_profils ON entreprises USING gin (profils);

-- ─── Contacts (personnes) ──────────────────────────────────────────────────
CREATE TABLE contacts (
    id                          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    civilite                    text CHECK (civilite IN ('M.','Mme.','Inconnu')),
    nom                         text NOT NULL,
    prenom                      text,
    entreprise_id               bigint REFERENCES entreprises (id) ON DELETE SET NULL,
    profils                     text[] NOT NULL DEFAULT '{}'
        CHECK (profils <@ ARRAY['prospect','client','fournisseur_materiaux','fournisseur_sous_traitant',
                                'co_traitant','architecte','autre','fournisseur_autres','ex_client','proprietaire']),
    numero_tva                  text,
    identifiant_comptable       text,
    franco_cents                bigint CHECK (franco_cents >= 0),
    rue                         text NOT NULL DEFAULT '',
    numero                      text NOT NULL DEFAULT '',
    code_postal                 text NOT NULL DEFAULT '',
    ville                       text NOT NULL DEFAULT '',
    pays                        text NOT NULL DEFAULT 'BE',
    facturation_identique       boolean NOT NULL DEFAULT true,
    facturation_rue             text,
    facturation_numero          text,
    facturation_code_postal     text,
    facturation_ville           text,
    facturation_pays            text,
    email                       text,
    email_bis                   text,
    telephone                   text,
    telephone_secondaire        text,
    site_web                    text,
    iban                        text,
    bic                         text,
    conditions_paiement_id      bigint REFERENCES conditions_paiement (id),
    taux_tva_id                 bigint REFERENCES taux_tva (id),
    source                      text CHECK (source IN ('apporteur_affaires','client','marketing','prospection',
                                                       'recommandation_client','reseaux_sociaux','site_web')),
    langue                      text NOT NULL DEFAULT 'fr' CHECK (langue IN ('fr','en','nl_be','nl')),
    description                 text,
    created_at                  timestamptz NOT NULL DEFAULT now(),
    updated_at                  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX contacts_nom ON contacts (lower(nom));
CREATE INDEX contacts_entreprise ON contacts (entreprise_id);
CREATE INDEX contacts_profils ON contacts USING gin (profils);
