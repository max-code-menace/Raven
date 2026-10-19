-- Archive légale des factures et des notes de crédit : le PDF et l'UBL (Peppol BIS 3.0) tels qu'émis.
-- La conservation dure 10 ans à compter du 1er janvier qui suit l'émission (art. 60 du Code TVA), avec la garantie de
-- l'authenticité de l'origine et de l'intégrité du contenu. Pour une facture structurée, le XML est l'original ; un PDF
-- regénéré plus tard (autre modèle, société ou client modifiés) ne le remplace pas.
-- Le document est fabriqué à la validation, jamais modifié ni supprimé ensuite ; son empreinte SHA-256 permet de
-- vérifier qu'il n'a pas changé. (Pas d'horodatage qualifié : la date est celle du serveur.)
-- Les factures reprises de Vertuoza n'ont pas d'archive ici : leur PDF d'origine est dans Vertuoza.
CREATE TABLE facture_documents (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    facture_id  bigint NOT NULL REFERENCES factures (id),
    genre       text NOT NULL CHECK (genre IN ('pdf', 'ubl')),
    nom         text NOT NULL,
    contenu     bytea NOT NULL,
    -- SHA-256 du contenu, en hexadécimal (64 caractères).
    empreinte   text NOT NULL CHECK (length(empreinte) = 64),
    cree_le     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (facture_id, genre)
);

CREATE FUNCTION facture_documents_proteger() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NOT EXISTS (SELECT 1 FROM factures WHERE id = NEW.facture_id AND numero IS NOT NULL) THEN
            RAISE EXCEPTION 'seule une facture numérotée a une archive';
        END IF;
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'un document archivé ne se modifie ni ne se supprime';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER facture_documents_immuables BEFORE INSERT OR UPDATE OR DELETE ON facture_documents
    FOR EACH ROW EXECUTE FUNCTION facture_documents_proteger();
