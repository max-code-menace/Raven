-- Raven, étape 2 : comptes en libre-service (inscription, vérification de l'e-mail, mot de passe oublié,
-- invitations, assistant de démarrage).
--
-- Les tables de comptes (utilisateurs, jetons, invitations, journal des e-mails du site) sont communes au site,
-- comme `membres` et `sessions` : elles servent avant de savoir dans quelle organisation on travaille (connexion,
-- liens reçus par e-mail). Les écrans qui les lisent pour une organisation (Réglages › Équipe) filtrent toujours
-- explicitement sur l'organisation de la session.

-- ─── Vérification de l'adresse e-mail ───────────────────────────────────────
-- Un compte inscrit sur le site ne se connecte qu'après avoir cliqué le lien reçu par e-mail.
ALTER TABLE utilisateurs ADD COLUMN email_verifie_le timestamptz;
-- Les comptes déjà créés (en ligne de commande) sont vérifiés.
UPDATE utilisateurs SET email_verifie_le = created_at;

-- Les liens envoyés par e-mail (vérification de l'adresse, nouveau mot de passe) : un jeton aléatoire dont la base
-- ne garde que l'empreinte SHA-256, à usage unique et limité dans le temps.
CREATE TABLE jetons_compte (
    empreinte       bytea PRIMARY KEY,
    genre           text NOT NULL CHECK (genre IN ('verification', 'mot_de_passe')),
    utilisateur_id  bigint NOT NULL REFERENCES utilisateurs (id) ON DELETE CASCADE,
    expire_le       timestamptz NOT NULL,
    utilise_le      timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX jetons_compte_utilisateur ON jetons_compte (utilisateur_id, genre);

-- Le journal des e-mails du site (pas ceux d'une organisation) : sans le texte, qui contient les liens secrets.
-- Sert aussi à limiter les envois répétés vers une même adresse.
CREATE TABLE emails_systeme (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    genre           text NOT NULL CHECK (genre IN ('verification', 'deja_inscrit', 'mot_de_passe', 'invitation')),
    destinataire    text NOT NULL,
    objet           text NOT NULL,
    statut          text NOT NULL CHECK (statut IN ('envoye', 'enregistre', 'echec')),
    erreur          text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX emails_systeme_destinataire ON emails_systeme (lower(destinataire), genre, created_at);

-- ─── Invitations dans une organisation ──────────────────────────────────────
CREATE TABLE invitations (
    id               bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organisation_id  bigint NOT NULL REFERENCES organisations (id),
    email            text NOT NULL,
    role             role_membre NOT NULL DEFAULT 'membre',
    empreinte        bytea NOT NULL UNIQUE,
    invite_par       bigint REFERENCES utilisateurs (id) ON DELETE SET NULL,
    expire_le        timestamptz NOT NULL,
    acceptee_le      timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now()
);
-- Une seule invitation en attente par adresse et par organisation.
CREATE UNIQUE INDEX invitations_en_attente ON invitations (organisation_id, lower(email)) WHERE acceptee_le IS NULL;

-- ─── Assistant de démarrage et profil métier ────────────────────────────────
-- `demarre_le` : l'assistant de démarrage est fait (vide : le propriétaire y est conduit à la connexion).
-- `profil_metier` : choisi à l'inscription ; réglera le vocabulaire, les modules (étape 3) et la bibliothèque (étape 4).
ALTER TABLE organisations
    ADD COLUMN demarre_le timestamptz,
    ADD COLUMN profil_metier text NOT NULL DEFAULT 'batiment'
        CHECK (profil_metier IN ('batiment', 'espaces_verts', 'peinture', 'graphisme', 'autre'));
-- Les organisations déjà en service (Ubique) n'ont pas d'assistant à faire.
UPDATE organisations SET demarre_le = created_at;

-- Le régime de TVA de la société : assujetti, ou franchise des petites entreprises (pas de TVA ; les mentions et les
-- calculs viendront à l'étape 3).
ALTER TABLE societe ADD COLUMN regime_tva text NOT NULL DEFAULT 'assujetti' CHECK (regime_tva IN ('assujetti', 'franchise'));
