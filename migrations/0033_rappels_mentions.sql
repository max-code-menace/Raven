-- Rappels de paiement conformes (livre XIX du Code de droit économique pour les particuliers, loi du 2 août 2002 pour
-- les entreprises). Trois réglages de l'artisan, tous éteints par défaut :
--  - ses conditions générales prévoient-elles une indemnité de retard pour les particuliers ? (sans clause expresse,
--    aucune indemnité ne peut être réclamée) ;
--  - le taux d'intérêt de retard de ses conditions générales envers les particuliers (vide = aucun) ;
--  - est-il une PME au sens de l'art. 1:24 du Code des sociétés et des associations ? Une PME peut faire courir ses
--    intérêts dès le début du délai de 14 jours, à condition de ne les réclamer qu'après son expiration sans paiement.
ALTER TABLE reglages
    ADD COLUMN rappel_indemnite_particuliers    boolean NOT NULL DEFAULT false,
    -- Centièmes de pour cent par an (1000 = 10 %) ; NULL : pas d'intérêts de retard envers les particuliers.
    ADD COLUMN rappel_interets_particuliers     integer CHECK (rappel_interets_particuliers BETWEEN 0 AND 3000),
    ADD COLUMN rappel_pme                       boolean NOT NULL DEFAULT false;
