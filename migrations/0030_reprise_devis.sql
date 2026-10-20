-- Reprise des données de Vertuoza (transition, étape 2 : devis, avenants, chantiers).
-- Un devis « repris » a été émis dans Vertuoza : son numéro, sa date, son statut et ses montants viennent de là.
ALTER TABLE devis ADD COLUMN reprise boolean NOT NULL DEFAULT false;
-- La référence du chantier dans Vertuoza (« PL45 ») : elle rattache les factures reprises (factures.chantier_vertuoza)
-- au chantier du devis de base.
ALTER TABLE devis ADD COLUMN chantier_vertuoza text NOT NULL DEFAULT '';
