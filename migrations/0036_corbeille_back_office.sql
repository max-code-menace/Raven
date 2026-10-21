-- Raven, étapes 4-2 à 4-4 : corbeille de la bibliothèque, back office de Raven, mises à jour du catalogue.

-- ─── 4-2 : la corbeille ─────────────────────────────────────────────────────
-- Un ouvrage supprimé n'est pas effacé : il va dans « Supprimés » (date de suppression) et se récupère. Il ne
-- disparaît jamais seul. Une copie d'un ouvrage du catalogue qui est dans la corbeille ne revient pas par une mise à
-- jour (sa ligne existe toujours, avec son `catalogue_ouvrage_id`).
ALTER TABLE ouvrages ADD COLUMN supprime_le timestamptz;

-- ─── 4-3 : les administrateurs de Raven (back office) ───────────────────────
-- Des comptes à part, communs au site, sans aucun lien avec les organisations : un administrateur n'est membre
-- d'aucune organisation et ne voit jamais leurs devis ni leurs factures. Créés en ligne de commande seulement
-- (`cargo run -- admin ajouter <e-mail>`). Session propre (cookie `raven_admin`, limité à /admin) ; la base ne garde
-- que l'empreinte du jeton.
CREATE TABLE administrateurs (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email           text NOT NULL CHECK (email <> ''),
    nom             text NOT NULL DEFAULT '',
    mot_de_passe    text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX administrateurs_email ON administrateurs (lower(email));

CREATE TABLE sessions_admin (
    empreinte           bytea PRIMARY KEY,
    administrateur_id   bigint NOT NULL REFERENCES administrateurs (id) ON DELETE CASCADE,
    expire_le           timestamptz NOT NULL,
    created_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE connexions_admin_ratees (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email       text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX connexions_admin_ratees_email ON connexions_admin_ratees (lower(email), created_at);

-- ─── 4-3 et 4-4 : modifier le catalogue, puis publier ───────────────────────
-- Un ouvrage ou une bibliothèque ajouté dans le back office attend la publication (`publie_le` vide) : les
-- organisations ne le voient qu'une fois publié. Une correction de texte vaut tout de suite pour les prochaines
-- copies ; elle ne change jamais un ouvrage déjà copié dans une organisation. Retirer un ouvrage ou une bibliothèque
-- ne fait que l'écarter des prochaines copies (les organisations gardent leurs copies).
ALTER TABLE catalogue_ouvrages
    ADD COLUMN publie_le timestamptz,
    ADD COLUMN retire_le timestamptz;
ALTER TABLE catalogue_bibliotheques
    ADD COLUMN publiee_le timestamptz,
    ADD COLUMN retiree_le timestamptz;
-- Le contenu de départ (migration 0035) est publié.
UPDATE catalogue_ouvrages SET publie_le = now();
UPDATE catalogue_bibliotheques SET publiee_le = now();

-- Le journal des publications.
CREATE TABLE catalogue_publications (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    publie_le           timestamptz NOT NULL DEFAULT now(),
    administrateur_id   bigint REFERENCES administrateurs (id) ON DELETE SET NULL,
    ouvrages            integer NOT NULL,
    bibliotheques       integer NOT NULL
);
