-- Connexion (avant la mise en ligne) : mot de passe Argon2 et session par cookie, comme Nodimmo.
-- Les utilisateurs se créent en ligne de commande (`cargo run -- utilisateur ajouter <e-mail>`), jamais depuis le site.

CREATE TABLE utilisateurs (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email           text NOT NULL,
    nom             text NOT NULL DEFAULT '',
    -- Argon2id, format PHC ($argon2id$v=19$...).
    mot_de_passe    text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX utilisateurs_email ON utilisateurs (lower(email));

-- Le cookie contient un jeton aléatoire ; la base ne garde que son empreinte SHA-256.
CREATE TABLE sessions (
    empreinte       bytea PRIMARY KEY,
    utilisateur_id  bigint NOT NULL REFERENCES utilisateurs (id) ON DELETE CASCADE,
    created_at      timestamptz NOT NULL DEFAULT now(),
    expire_le       timestamptz NOT NULL
);
CREATE INDEX sessions_utilisateur ON sessions (utilisateur_id);

-- Les essais ratés, pour freiner qui devinerait un mot de passe (5 essais par quart d'heure et par e-mail).
CREATE TABLE connexions_ratees (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email       text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX connexions_ratees_email ON connexions_ratees (lower(email), created_at);
