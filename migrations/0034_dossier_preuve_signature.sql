-- Dossier de preuve de la signature en ligne d'un devis (signature électronique « simple renforcée », eIDAS art. 25).
-- La preuve doit pouvoir répondre à « ce n'est pas moi qui ai signé » : quel document exactement (empreinte SHA-256 et
-- copie du PDF), quand (heure du serveur), avec quelle adresse e-mail et si le client a prouvé qu'il la contrôle (code à usage
-- unique reçu par e-mail), depuis quelle adresse IP et quel navigateur, et ce que l'écran lui a fait accepter (texte exact).
-- Le dossier est écrit une fois et ne change plus (déclencheur ci-dessous). Pas d'horodatage qualifié : l'heure est celle
-- du serveur ; au-delà de 3 500 € face à un particulier, une signature qualifiée (itsme) reste recommandée.
-- Les deux tables appartiennent à une organisation, comme toute table de données (voir la migration 0026).
CREATE TABLE devis_signatures (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organisation_id     bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    devis_id            bigint NOT NULL UNIQUE,
    signe_le            timestamptz NOT NULL DEFAULT now(),
    nom                 text NOT NULL,
    -- L'adresse e-mail du client sur le devis ; vide quand il n'en a pas.
    email               text NOT NULL DEFAULT '',
    -- code_par_email : le client a saisi le code reçu à cette adresse ; mode_test : aucune clé d'envoi (rien n'est parti,
    -- adresse non vérifiée) ; sans_email : le client n'a pas d'adresse sur le devis.
    verification        text NOT NULL CHECK (verification IN ('code_par_email', 'mode_test', 'sans_email')),
    ip                  text NOT NULL DEFAULT '',
    navigateur          text NOT NULL DEFAULT '',
    -- Le texte exact de l'acceptation affiché au client.
    texte_accepte       text NOT NULL,
    montant_ttc_cents   bigint NOT NULL,
    pdf_nom             text NOT NULL,
    pdf                 bytea NOT NULL,
    empreinte_pdf       text NOT NULL CHECK (length(empreinte_pdf) = 64),
    FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id)
);

CREATE FUNCTION devis_signatures_proteger() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'un dossier de signature ne se modifie ni ne se supprime';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER devis_signatures_immuables BEFORE UPDATE OR DELETE ON devis_signatures
    FOR EACH ROW EXECUTE FUNCTION devis_signatures_proteger();

-- Les codes à usage unique envoyés par e-mail avant de signer : valables 10 minutes, 5 essais, 3 envois par quart d'heure.
-- On ne garde que l'empreinte du code, jamais le code.
CREATE TABLE devis_codes_signature (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organisation_id bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    devis_id        bigint NOT NULL,
    email           text NOT NULL,
    empreinte_code  text NOT NULL,
    expire_le       timestamptz NOT NULL,
    essais          integer NOT NULL DEFAULT 0,
    utilise         boolean NOT NULL DEFAULT false,
    created_at      timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id) ON DELETE CASCADE
);
CREATE INDEX devis_codes_signature_devis ON devis_codes_signature (devis_id, created_at DESC);

ALTER TABLE devis_signatures ENABLE ROW LEVEL SECURITY;
ALTER TABLE devis_signatures FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON devis_signatures USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());
ALTER TABLE devis_codes_signature ENABLE ROW LEVEL SECURITY;
ALTER TABLE devis_codes_signature FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON devis_codes_signature USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());
