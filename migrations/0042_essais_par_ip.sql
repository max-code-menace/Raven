-- Mise en ligne (étape 7-1) : les essais par adresse IP (connexion ratée, inscription, mot de passe oublié, connexion
-- ratée au back office), en plus des limites par adresse e-mail. Table commune au site, comme `connexions_ratees` : une
-- adresse IP n'appartient à aucune organisation. Les lignes de plus d'un jour sont effacées (`securite::noter`).
CREATE TABLE essais_ip (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ip          text NOT NULL CHECK (ip <> '' AND length(ip) <= 45),
    genre       text NOT NULL CHECK (genre IN ('connexion', 'connexion_admin', 'inscription', 'mot_de_passe')),
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX essais_ip_compte ON essais_ip (genre, ip, created_at);
CREATE INDEX essais_ip_age ON essais_ip (created_at);
