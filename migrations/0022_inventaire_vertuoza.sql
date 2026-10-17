-- Inventaire de Vertuoza du 17/10/2026 : ce qu'Ubique utilise et qui manquait encore.

-- 1. Numérotation annuelle des factures et des notes de crédit.
-- Réglage relevé dans Vertuoza (Réglages › Factures › Préférences) : « Facturation annuelle : la numérotation
-- repassera à 1 à chaque nouvelle année fiscale » (années fiscales = années civiles). Constaté dans la liste :
-- facture n° 106 du 02/12/2025, puis la numérotation 2026 repart de 1 (dernière : n° 124).
-- `annee` : l'année de la dernière pièce numérotée de la série ; une pièce d'une année suivante repart de 1.
-- Les compteurs (124 factures, 5 notes de crédit, repris de Vertuoza) sont ceux de l'année en cours : 2026 pour la
-- vraie base, migrée en 2026. À revoir si la base est créée à neuf une autre année (voir aussi la migration 0013).
ALTER TABLE compteurs ADD COLUMN annee integer;
UPDATE compteurs SET annee = EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int WHERE nom IN ('facture', 'note_credit');

-- Un numéro est unique dans sa série et dans son année.
DROP INDEX factures_numero;
DROP INDEX notes_credit_numero;
CREATE UNIQUE INDEX factures_numero ON factures ((EXTRACT(YEAR FROM date_facture)), numero)
    WHERE genre <> 'note_credit' AND numero IS NOT NULL;
CREATE UNIQUE INDEX notes_credit_numero ON factures ((EXTRACT(YEAR FROM date_facture)), numero)
    WHERE genre = 'note_credit' AND numero IS NOT NULL;

-- 2. Factures récurrentes (Ubique : location mensuelle de deux lots, « Location - Lot 1 / Lot 2 »).
-- Dans Vertuoza : type « Facture récurrente » et, sur une facture, « Créer/voir la récurrence » (date de début,
-- date de fin, type journalière / hebdomadaire / mensuelle / annuelle, « tous les N », « créer la facture le jour
-- même ou N jours avant la date prévue »).
ALTER TYPE genre_facture ADD VALUE 'recurrente' BEFORE 'note_credit';

CREATE TABLE recurrences (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- La facture qui sert de modèle (en-tête et lignes recopiés à chaque échéance).
    modele_id       bigint NOT NULL UNIQUE REFERENCES factures (id) ON DELETE CASCADE,
    date_debut      date NOT NULL,
    date_fin        date NOT NULL CHECK (date_fin >= date_debut AND date_fin <= date_debut + 3660),
    frequence       text NOT NULL CHECK (frequence IN ('jour', 'semaine', 'mois', 'annee')),
    intervalle      integer NOT NULL DEFAULT 1 CHECK (intervalle BETWEEN 1 AND 365),
    -- Le brouillon est préparé ce nombre de jours avant la date prévue (0 : le jour même).
    jours_avance    integer NOT NULL DEFAULT 0 CHECK (jours_avance BETWEEN 0 AND 60),
    -- La dernière date prévue déjà préparée : une échéance préparée n'est plus jamais proposée (même si son
    -- brouillon a été supprimé).
    preparee_jusqua date,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Chaque facture créée par une récurrence garde sa date prévue : jamais deux factures pour la même échéance.
ALTER TABLE factures ADD COLUMN recurrence_id bigint REFERENCES recurrences (id) ON DELETE SET NULL;
ALTER TABLE factures ADD COLUMN date_prevue date;
CREATE UNIQUE INDEX factures_echeance_recurrence ON factures (recurrence_id, date_prevue) WHERE recurrence_id IS NOT NULL;

-- 3. Chaque ouverture d'un devis ou d'une facture par le client (Vertuoza : notification « La facture … a été
-- ouverte » à chaque ouverture). Affiché sur le tableau de bord.
CREATE TABLE consultations_client (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    devis_id    bigint REFERENCES devis (id) ON DELETE CASCADE,
    facture_id  bigint REFERENCES factures (id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK ((devis_id IS NULL) <> (facture_id IS NULL))
);
CREATE INDEX consultations_client_date ON consultations_client (created_at DESC);

-- 4. Les modèles d'e-mails qu'Ubique utilise par défaut dans Vertuoza (Réglages › Modèles › E-mails), repris mot
-- pour mot. Les rappels de paiement d'Ubique n'y sont pas activés : nos rappels gardent leurs textes.
UPDATE modeles_email SET updated_at = now(), objet = $$Votre devis [NUMBER_QUOTE] – [REFERENCE]$$, corps = $$Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Suite à nos échanges, nous avons le plaisir de vous transmettre le devis n° [NUMBER_QUOTE] (« [REFERENCE] »).
Vous pouvez le consulter en cliquant sur ce lien : [LINK_DEVIS]

Si cette proposition vous convient, merci de nous retourner le devis daté et signé : nous pourrons alors passer aux étapes suivantes.

Pour toute question, n’hésitez pas à répondre à ce mail ou à nous contacter.

Bien cordialement,
[NOM_GESTIONNAIRE]
[MAIL_GESTIONNAIRE]$$ WHERE code = 'devis';

UPDATE modeles_email SET updated_at = now(), objet = $$Avenant au devis – chantier [NOM_CHANTIER]$$, corps = $$Bonjour [NOM_CLIENT],

Suite à nos échanges, veuillez trouver ci-joint l’avenant au devis pour le chantier [NOM_CHANTIER]. Il détaille les travaux supplémentaires et leur coût.
Vous pouvez le consulter en cliquant sur ce lien : [LINK_DEVIS]

Si cet avenant vous convient, merci de nous le retourner daté et signé : nous pourrons ainsi poursuivre les travaux sans interruption.

Pour toute question, répondez simplement à ce mail ou contactez-nous.

Bien cordialement,
[NOM_GESTIONNAIRE]$$ WHERE code = 'avenant';

UPDATE modeles_email SET updated_at = now(), objet = $$Facture [NO_FACTURE] – chantier [NOM_CHANTIER]$$, corps = $$Bonjour [NOM_CLIENT],

Veuillez trouver ci-joint la facture n° [NO_FACTURE] du [DATE_FACTURE], relative au chantier [NOM_CHANTIER] (« [REFERENCE] »).
Vous pouvez également la consulter en ligne : [LINK_FACTURE]

Nous vous remercions de bien vouloir procéder au règlement selon les conditions de paiement convenues. Les coordonnées bancaires figurent sur la facture.

Pour toute question concernant cette facture, n’hésitez pas à répondre à ce mail ou à nous contacter.

Nous vous remercions pour votre confiance.

Bien cordialement,
[NOM_GESTIONNAIRE]
[MAIL_GESTIONNAIRE]$$ WHERE code = 'facture';
