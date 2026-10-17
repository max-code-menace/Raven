-- Reprise des données de Vertuoza (transition, étape 1 : factures et paiements de 2026).
-- Une facture « reprise » a été émise par Vertuoza : son numéro, ses montants et son PDF d'origine font foi.
-- Elle est verrouillée comme toute facture validée, compte dans les impayés, les rappels et les statistiques,
-- mais pas dans l'export pour le comptable (il l'a déjà reçue de Vertuoza).
ALTER TABLE factures ADD COLUMN reprise boolean NOT NULL DEFAULT false;
-- La référence du chantier dans Vertuoza (« PL45 »), pour rattacher la facture à son chantier à l'étape 2.
ALTER TABLE factures ADD COLUMN chantier_vertuoza text NOT NULL DEFAULT '';

-- Une facture d'acompte, d'avancement ou finale reprise n'a pas encore de chantier ni d'avancement dans Ubifacture
-- (ils arrivent aux étapes 2 et 3) : les contrôles ne s'appliquent qu'aux factures créées dans Ubifacture.
DO $$
DECLARE c record;
BEGIN
    FOR c IN SELECT conname FROM pg_constraint
             WHERE conrelid = 'factures'::regclass AND contype = 'c'
               AND (pg_get_constraintdef(oid) LIKE '%avancement_id%' OR pg_get_constraintdef(oid) LIKE '%chantier_id%')
    LOOP
        EXECUTE 'ALTER TABLE factures DROP CONSTRAINT ' || quote_ident(c.conname);
    END LOOP;
END $$;

ALTER TABLE factures ADD CONSTRAINT factures_chantier_requis
    CHECK (reprise OR genre NOT IN ('acompte', 'avancement', 'finale') OR chantier_id IS NOT NULL);
ALTER TABLE factures ADD CONSTRAINT factures_avancement_requis
    CHECK (reprise OR genre NOT IN ('avancement', 'finale') OR avancement_id IS NOT NULL);
ALTER TABLE factures ADD CONSTRAINT factures_avancement_seulement
    CHECK (genre IN ('avancement', 'finale') OR avancement_id IS NULL);
