-- Raven, étape 1-2 : chaque donnée appartient à une organisation, et PostgreSQL ne montre à une page que les lignes
-- de son organisation (sécurité au niveau des lignes, RLS).
--
-- Principe (voir `src/base.rs`) : chaque page règle `app.organisation_id` sur sa connexion. Les règles ci-dessous ne
-- laissent lire et écrire que les lignes de cette organisation. **Fermé par défaut** : sans réglage, aucune ligne n'est
-- visible et aucune écriture n'est acceptée (la colonne `organisation_id` prend la valeur du réglage, NOT NULL).
-- `FORCE` : les règles s'appliquent aussi au propriétaire des tables (l'utilisateur `ubifacture`). Seul un
-- super-utilisateur les contourne : l'application ne doit jamais se connecter avec un tel compte.
--
-- ATTENTION pour les migrations suivantes : une migration qui lit ou modifie des données ne voit rien (aucune
-- organisation n'est réglée pendant les migrations). Elle doit régler l'organisation (set_config) pour chacune, ou
-- retirer `FORCE` le temps de la migration, dans la même transaction.
--
-- Toutes les données existantes appartiennent à Ubique (organisation n° 1).

-- L'organisation de la connexion (NULL si aucune).
CREATE FUNCTION app_organisation() RETURNS bigint
    LANGUAGE sql STABLE PARALLEL SAFE
    AS $$ SELECT nullif(current_setting('app.organisation_id', true), '')::bigint $$;

-- ─── 1. La colonne organisation_id ──────────────────────────────────────────
-- Ajoutée avec la valeur 1 pour les lignes existantes (sans réécrire les lignes, donc sans déclencher les
-- garde-fous des factures validées), puis la valeur par défaut devient l'organisation de la connexion.
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'entreprises', 'contacts', 'devis', 'devis_lignes', 'devis_evenements', 'chantiers',
        'avancements', 'avancement_lignes', 'factures', 'facture_lignes', 'facture_evenements', 'paiements',
        'emails', 'exports', 'export_pieces', 'chantier_achats', 'ouvrages', 'imports_excel',
        'recurrences', 'consultations_client', 'societe', 'societe_comptes_bancaires', 'societe_fichiers', 'reglages',
        'modeles_email', 'compteurs'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ADD COLUMN organisation_id bigint NOT NULL DEFAULT 1 REFERENCES organisations (id)', t);
        EXECUTE format('ALTER TABLE %I ALTER COLUMN organisation_id SET DEFAULT app_organisation()', t);
    END LOOP;
END $$;

-- ─── 2. Ce qui n'existait qu'une fois pour tout le site existe une fois par organisation ───────────────
ALTER TABLE societe DROP COLUMN id, ADD PRIMARY KEY (organisation_id);
ALTER TABLE reglages DROP COLUMN id, ADD PRIMARY KEY (organisation_id);
ALTER TABLE compteurs DROP CONSTRAINT compteurs_pkey, ADD PRIMARY KEY (organisation_id, nom);
ALTER TABLE modeles_email DROP CONSTRAINT modeles_email_pkey, ADD PRIMARY KEY (organisation_id, code);
ALTER TABLE societe_fichiers DROP CONSTRAINT societe_fichiers_pkey, ADD PRIMARY KEY (organisation_id, usage);
DROP INDEX societe_un_seul_compte_par_defaut;
CREATE UNIQUE INDEX societe_un_seul_compte_par_defaut ON societe_comptes_bancaires (organisation_id) WHERE par_defaut;

-- ─── 3. Les numéros sont uniques dans une organisation (chacune a sa numérotation) ─────────────────
DROP INDEX devis_numero_unique;
CREATE UNIQUE INDEX devis_numero_unique ON devis (organisation_id, numero) WHERE chantier_id IS NULL;
ALTER TABLE chantiers DROP CONSTRAINT chantiers_numero_key, ADD CONSTRAINT chantiers_numero_key UNIQUE (organisation_id, numero);
DROP INDEX factures_numero;
CREATE UNIQUE INDEX factures_numero ON factures (organisation_id, (EXTRACT(YEAR FROM date_facture)), numero)
    WHERE genre <> 'note_credit' AND numero IS NOT NULL;
DROP INDEX notes_credit_numero;
CREATE UNIQUE INDEX notes_credit_numero ON factures (organisation_id, (EXTRACT(YEAR FROM date_facture)), numero)
    WHERE genre = 'note_credit' AND numero IS NOT NULL;

-- ─── 4. Une ligne ne peut pointer que vers une ligne de la même organisation ───────────────────────
-- (exemple : une facture ne peut pas désigner le client d'une autre organisation). Chaque lien devient un lien
-- sur (organisation_id, id), avec le même comportement à la suppression qu'avant.
ALTER TABLE avancements ADD CONSTRAINT avancements_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE chantiers ADD CONSTRAINT chantiers_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE contacts ADD CONSTRAINT contacts_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE devis ADD CONSTRAINT devis_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE devis_lignes ADD CONSTRAINT devis_lignes_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE entreprises ADD CONSTRAINT entreprises_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE exports ADD CONSTRAINT exports_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE factures ADD CONSTRAINT factures_organisation_id_key UNIQUE (organisation_id, id);
ALTER TABLE recurrences ADD CONSTRAINT recurrences_organisation_id_key UNIQUE (organisation_id, id);

ALTER TABLE contacts DROP CONSTRAINT contacts_entreprise_id_fkey,
    ADD CONSTRAINT contacts_entreprise_id_fkey FOREIGN KEY (organisation_id, entreprise_id) REFERENCES entreprises (organisation_id, id) ON DELETE SET NULL (entreprise_id);
ALTER TABLE devis DROP CONSTRAINT devis_architecte_contact_id_fkey,
    ADD CONSTRAINT devis_architecte_contact_id_fkey FOREIGN KEY (organisation_id, architecte_contact_id) REFERENCES contacts (organisation_id, id);
ALTER TABLE devis DROP CONSTRAINT devis_chantier_id_fkey,
    ADD CONSTRAINT devis_chantier_id_fkey FOREIGN KEY (organisation_id, chantier_id) REFERENCES chantiers (organisation_id, id);
ALTER TABLE devis DROP CONSTRAINT devis_client_contact_id_fkey,
    ADD CONSTRAINT devis_client_contact_id_fkey FOREIGN KEY (organisation_id, client_contact_id) REFERENCES contacts (organisation_id, id);
ALTER TABLE devis DROP CONSTRAINT devis_client_entreprise_id_fkey,
    ADD CONSTRAINT devis_client_entreprise_id_fkey FOREIGN KEY (organisation_id, client_entreprise_id) REFERENCES entreprises (organisation_id, id);
ALTER TABLE devis DROP CONSTRAINT devis_facturation_contact_id_fkey,
    ADD CONSTRAINT devis_facturation_contact_id_fkey FOREIGN KEY (organisation_id, facturation_contact_id) REFERENCES contacts (organisation_id, id);
ALTER TABLE devis DROP CONSTRAINT devis_facturation_entreprise_id_fkey,
    ADD CONSTRAINT devis_facturation_entreprise_id_fkey FOREIGN KEY (organisation_id, facturation_entreprise_id) REFERENCES entreprises (organisation_id, id);
ALTER TABLE devis DROP CONSTRAINT devis_gestionnaire_contact_id_fkey,
    ADD CONSTRAINT devis_gestionnaire_contact_id_fkey FOREIGN KEY (organisation_id, gestionnaire_contact_id) REFERENCES contacts (organisation_id, id);
ALTER TABLE devis_lignes DROP CONSTRAINT devis_lignes_devis_id_fkey,
    ADD CONSTRAINT devis_lignes_devis_id_fkey FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE devis_evenements DROP CONSTRAINT devis_evenements_devis_id_fkey,
    ADD CONSTRAINT devis_evenements_devis_id_fkey FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE chantiers DROP CONSTRAINT chantiers_devis_id_fkey,
    ADD CONSTRAINT chantiers_devis_id_fkey FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id);
ALTER TABLE avancements DROP CONSTRAINT avancements_chantier_id_fkey,
    ADD CONSTRAINT avancements_chantier_id_fkey FOREIGN KEY (organisation_id, chantier_id) REFERENCES chantiers (organisation_id, id);
ALTER TABLE avancement_lignes DROP CONSTRAINT avancement_lignes_ajustement_devis_id_fkey,
    ADD CONSTRAINT avancement_lignes_ajustement_devis_id_fkey FOREIGN KEY (organisation_id, ajustement_devis_id) REFERENCES devis (organisation_id, id);
ALTER TABLE avancement_lignes DROP CONSTRAINT avancement_lignes_avancement_id_fkey,
    ADD CONSTRAINT avancement_lignes_avancement_id_fkey FOREIGN KEY (organisation_id, avancement_id) REFERENCES avancements (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE avancement_lignes DROP CONSTRAINT avancement_lignes_devis_ligne_id_fkey,
    ADD CONSTRAINT avancement_lignes_devis_ligne_id_fkey FOREIGN KEY (organisation_id, devis_ligne_id) REFERENCES devis_lignes (organisation_id, id);
ALTER TABLE factures DROP CONSTRAINT factures_avancement_id_fkey,
    ADD CONSTRAINT factures_avancement_id_fkey FOREIGN KEY (organisation_id, avancement_id) REFERENCES avancements (organisation_id, id);
ALTER TABLE factures DROP CONSTRAINT factures_chantier_id_fkey,
    ADD CONSTRAINT factures_chantier_id_fkey FOREIGN KEY (organisation_id, chantier_id) REFERENCES chantiers (organisation_id, id);
ALTER TABLE factures DROP CONSTRAINT factures_client_contact_id_fkey,
    ADD CONSTRAINT factures_client_contact_id_fkey FOREIGN KEY (organisation_id, client_contact_id) REFERENCES contacts (organisation_id, id);
ALTER TABLE factures DROP CONSTRAINT factures_client_entreprise_id_fkey,
    ADD CONSTRAINT factures_client_entreprise_id_fkey FOREIGN KEY (organisation_id, client_entreprise_id) REFERENCES entreprises (organisation_id, id);
ALTER TABLE factures DROP CONSTRAINT factures_facture_origine_id_fkey,
    ADD CONSTRAINT factures_facture_origine_id_fkey FOREIGN KEY (organisation_id, facture_origine_id) REFERENCES factures (organisation_id, id);
ALTER TABLE factures DROP CONSTRAINT factures_recurrence_id_fkey,
    ADD CONSTRAINT factures_recurrence_id_fkey FOREIGN KEY (organisation_id, recurrence_id) REFERENCES recurrences (organisation_id, id) ON DELETE SET NULL (recurrence_id);
ALTER TABLE facture_lignes DROP CONSTRAINT facture_lignes_facture_id_fkey,
    ADD CONSTRAINT facture_lignes_facture_id_fkey FOREIGN KEY (organisation_id, facture_id) REFERENCES factures (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE paiements DROP CONSTRAINT paiements_facture_id_fkey,
    ADD CONSTRAINT paiements_facture_id_fkey FOREIGN KEY (organisation_id, facture_id) REFERENCES factures (organisation_id, id);
ALTER TABLE facture_evenements DROP CONSTRAINT facture_evenements_facture_id_fkey,
    ADD CONSTRAINT facture_evenements_facture_id_fkey FOREIGN KEY (organisation_id, facture_id) REFERENCES factures (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE emails DROP CONSTRAINT emails_devis_id_fkey,
    ADD CONSTRAINT emails_devis_id_fkey FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id) ON DELETE SET NULL (devis_id);
ALTER TABLE emails DROP CONSTRAINT emails_export_id_fkey,
    ADD CONSTRAINT emails_export_id_fkey FOREIGN KEY (organisation_id, export_id) REFERENCES exports (organisation_id, id) ON DELETE SET NULL (export_id);
ALTER TABLE emails DROP CONSTRAINT emails_facture_id_fkey,
    ADD CONSTRAINT emails_facture_id_fkey FOREIGN KEY (organisation_id, facture_id) REFERENCES factures (organisation_id, id) ON DELETE SET NULL (facture_id);
ALTER TABLE export_pieces DROP CONSTRAINT export_pieces_export_id_fkey,
    ADD CONSTRAINT export_pieces_export_id_fkey FOREIGN KEY (organisation_id, export_id) REFERENCES exports (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE export_pieces DROP CONSTRAINT export_pieces_facture_id_fkey,
    ADD CONSTRAINT export_pieces_facture_id_fkey FOREIGN KEY (organisation_id, facture_id) REFERENCES factures (organisation_id, id);
ALTER TABLE chantier_achats DROP CONSTRAINT chantier_achats_chantier_id_fkey,
    ADD CONSTRAINT chantier_achats_chantier_id_fkey FOREIGN KEY (organisation_id, chantier_id) REFERENCES chantiers (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE imports_excel DROP CONSTRAINT imports_excel_devis_id_fkey,
    ADD CONSTRAINT imports_excel_devis_id_fkey FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE recurrences DROP CONSTRAINT recurrences_modele_id_fkey,
    ADD CONSTRAINT recurrences_modele_id_fkey FOREIGN KEY (organisation_id, modele_id) REFERENCES factures (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE consultations_client DROP CONSTRAINT consultations_client_devis_id_fkey,
    ADD CONSTRAINT consultations_client_devis_id_fkey FOREIGN KEY (organisation_id, devis_id) REFERENCES devis (organisation_id, id) ON DELETE CASCADE;
ALTER TABLE consultations_client DROP CONSTRAINT consultations_client_facture_id_fkey,
    ADD CONSTRAINT consultations_client_facture_id_fkey FOREIGN KEY (organisation_id, facture_id) REFERENCES factures (organisation_id, id) ON DELETE CASCADE;

-- Index pour les listes des tables sans lien entrant (les autres ont déjà (organisation_id, id)).
CREATE INDEX ouvrages_organisation ON ouvrages (organisation_id);
CREATE INDEX paiements_organisation ON paiements (organisation_id);
CREATE INDEX emails_organisation ON emails (organisation_id);
CREATE INDEX chantier_achats_organisation ON chantier_achats (organisation_id);
CREATE INDEX consultations_client_organisation ON consultations_client (organisation_id);

-- ─── 5. Les règles de sécurité ──────────────────────────────────────────────
DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'entreprises', 'contacts', 'devis', 'devis_lignes', 'devis_evenements', 'chantiers',
        'avancements', 'avancement_lignes', 'factures', 'facture_lignes', 'facture_evenements', 'paiements',
        'emails', 'exports', 'export_pieces', 'chantier_achats', 'ouvrages', 'imports_excel',
        'recurrences', 'consultations_client', 'societe', 'societe_comptes_bancaires', 'societe_fichiers', 'reglages',
        'modeles_email', 'compteurs'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', t);
        EXECUTE format('CREATE POLICY par_organisation ON %I USING (organisation_id = app_organisation()) '
                       'WITH CHECK (organisation_id = app_organisation())', t);
    END LOOP;
END $$;

-- La page client en ligne (sans connexion) : avec le seul lien secret, on peut lire la pièce de ce lien, et rien
-- d'autre, pour connaître son organisation (voir `page_client::base_du_jeton`). Le réglage `app.jeton` n'est posé
-- que le temps de cette lecture.
CREATE POLICY lien_secret ON devis FOR SELECT USING (jeton = nullif(current_setting('app.jeton', true), ''));
CREATE POLICY lien_secret ON factures FOR SELECT USING (jeton = nullif(current_setting('app.jeton', true), ''));

-- La vue des états de paiement applique les règles de celui qui la lit.
ALTER VIEW factures_etats SET (security_invoker = true);
