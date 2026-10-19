-- Raven, étape 3 : un produit pour tous les métiers.

-- ─── Vocabulaire et modules de chaque organisation ──────────────────────────
-- `vocabulaire` : « chantier » (bâtiment) ou « projet » (les autres métiers) dans les écrans et les PDF.
-- Les modules s'allument ou s'éteignent dans Réglages › Métier et modules ; éteint, un module disparaît des écrans
-- (les données déjà saisies restent). L'assistant de démarrage les règle selon le métier choisi.
-- `module_import_metres` : l'import du modèle Excel de métré (celui d'Ubique), éteint par défaut.
ALTER TABLE organisations
    ADD COLUMN vocabulaire text NOT NULL DEFAULT 'chantier' CHECK (vocabulaire IN ('chantier', 'projet')),
    ADD COLUMN module_avancements boolean NOT NULL DEFAULT true,
    ADD COLUMN module_avenants boolean NOT NULL DEFAULT true,
    ADD COLUMN module_achats boolean NOT NULL DEFAULT true,
    ADD COLUMN module_import_metres boolean NOT NULL DEFAULT false;
-- Les organisations déjà en service (Ubique) gardent tout ce qu'elles utilisent.
UPDATE organisations SET module_import_metres = true WHERE demarre_le IS NOT NULL;

-- ─── TVA : taux de 12 % et franchise des petites entreprises ────────────────
-- `franchise` : le « taux » des entreprises sous le régime de franchise (pas de TVA, mention obligatoire sur les
-- documents). Seul taux proposé à une société en franchise, jamais proposé aux autres.
ALTER TABLE taux_tva ADD COLUMN franchise boolean NOT NULL DEFAULT false;
ALTER TABLE taux_tva ADD CONSTRAINT taux_tva_franchise_sans_tva CHECK (NOT franchise OR (taux_centiemes = 0 AND NOT autoliquidation));
INSERT INTO taux_tva (libelle, taux_centiemes, autoliquidation, franchise) VALUES
    ('Taux intermédiaire', 1200, false, false),
    ('Franchise de TVA', 0, false, true);


-- ─── Modèles d'e-mails de départ sans vocabulaire du bâtiment ───────────────
UPDATE modeles_email_defaut SET objet = 'Avenant n° [NUMBER_QUOTE] - [REFERENCE]', corps =
'Bonjour [CIVILITE_CLIENT] [NOM_CLIENT],

Veuillez trouver en pièce jointe l''avenant n° [NUMBER_QUOTE] à notre devis « [NOM_CHANTIER] ».
Vous pouvez aussi le consulter et le signer en ligne : [LINK_DEVIS]

Bien à vous,
[NOM_GESTIONNAIRE]
[NOM_SOCIETE] · [MAIL_GESTIONNAIRE]'
WHERE code = 'avenant';
