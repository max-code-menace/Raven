-- Raven, étape 4-1 : les bibliothèques de Raven (décisions de Mahx du 20/10/2026).
--
-- Un **catalogue commun** à tout le site (tables `catalogue_…`, sans `organisation_id` : aucune organisation ne le
-- modifie ; le back office de l'étape 4-3 le fera), rangé en arbre métier › bibliothèque. Il ne contient **aucun prix** :
-- seulement la description, le type de ligne et l'unité proposée.
--
-- Une organisation **coche** des bibliothèques (`organisation_bibliotheques`) : leurs ouvrages sont **copiés** dans sa
-- propre bibliothèque (`ouvrages`), sans prix (prix de vente vide = « à compléter »), et ils sont à elle seule.
-- `ouvrages.catalogue_ouvrage_id` garde l'origine d'une copie (jamais deux copies du même ouvrage) ; `personnalise`
-- passe à vrai dès que l'utilisateur change l'ouvrage. Décocher retire les copies non personnalisées et garde les autres.

CREATE TABLE catalogue_metiers (
    code        text PRIMARY KEY,
    nom         text NOT NULL,
    position    integer NOT NULL
);

CREATE TABLE catalogue_bibliotheques (
    code        text PRIMARY KEY,
    metier      text NOT NULL REFERENCES catalogue_metiers (code),
    nom         text NOT NULL CHECK (nom <> ''),
    position    integer NOT NULL
);

CREATE TABLE catalogue_ouvrages (
    id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    bibliotheque    text NOT NULL REFERENCES catalogue_bibliotheques (code),
    position        integer NOT NULL,
    description     text NOT NULL CHECK (description <> ''),
    genre           genre_ligne NOT NULL
        CHECK (genre::text IN ('poste_libre', 'ouvrage', 'fourniture', 'equipement', 'main_oeuvre', 'sous_traitant')),
    unite           text NOT NULL DEFAULT '',
    UNIQUE (bibliotheque, position)
);

-- Les bibliothèques cochées par chaque organisation (table de données : RLS, comme toutes, voir la migration 0026).
CREATE TABLE organisation_bibliotheques (
    organisation_id bigint NOT NULL DEFAULT app_organisation() REFERENCES organisations (id),
    bibliotheque    text NOT NULL REFERENCES catalogue_bibliotheques (code),
    cochee_le       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organisation_id, bibliotheque)
);
ALTER TABLE organisation_bibliotheques ENABLE ROW LEVEL SECURITY;
ALTER TABLE organisation_bibliotheques FORCE ROW LEVEL SECURITY;
CREATE POLICY par_organisation ON organisation_bibliotheques
    USING (organisation_id = app_organisation()) WITH CHECK (organisation_id = app_organisation());

-- Le prix de vente d'un ouvrage peut rester vide (« Prix à compléter ») : jamais 0 € par défaut.
ALTER TABLE ouvrages
    ALTER COLUMN prix_vente_e5 DROP NOT NULL,
    ADD COLUMN catalogue_ouvrage_id bigint REFERENCES catalogue_ouvrages (id) ON DELETE SET NULL,
    ADD COLUMN personnalise boolean NOT NULL DEFAULT false;
CREATE UNIQUE INDEX ouvrages_catalogue ON ouvrages (organisation_id, catalogue_ouvrage_id) WHERE catalogue_ouvrage_id IS NOT NULL;

-- ─── Le contenu de départ (listes relues par Mahx le 20/10/2026) ────────────
INSERT INTO catalogue_metiers (code, nom, position) VALUES
    ('batiment', 'Bâtiment', 1),
    ('espaces_verts', 'Espaces verts', 2),
    ('peinture', 'Peinture', 3),
    ('graphisme', 'Graphisme', 4);

INSERT INTO catalogue_bibliotheques (code, metier, nom, position) VALUES
    ('batiment.maconnerie', 'batiment', 'Maçonnerie et gros œuvre', 1),
    ('batiment.gyproc', 'batiment', 'Gyproc et plafonnage', 2),
    ('batiment.menuiserie', 'batiment', 'Menuiserie', 3),
    ('batiment.electricite', 'batiment', 'Électricité', 4),
    ('batiment.plomberie', 'batiment', 'Plomberie et chauffage', 5),
    ('batiment.toiture', 'batiment', 'Toiture', 6),
    ('batiment.carrelage', 'batiment', 'Carrelage', 7),
    ('espaces_verts.entretien', 'espaces_verts', 'Entretien de jardin', 8),
    ('espaces_verts.elagage', 'espaces_verts', 'Élagage et abattage', 9),
    ('espaces_verts.amenagement', 'espaces_verts', 'Aménagement', 10),
    ('peinture.peinture', 'peinture', 'Peinture', 11),
    ('graphisme.graphisme', 'graphisme', 'Graphisme', 12);

-- Maçonnerie et gros œuvre
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('batiment.maconnerie', 1, 'Installation et repli de chantier', 'ouvrage', 'forfait'),
    ('batiment.maconnerie', 2, 'Protection des sols et des abords', 'ouvrage', 'forfait'),
    ('batiment.maconnerie', 3, 'Démolition de maçonnerie, évacuation comprise', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 4, 'Démolition de dalle en béton', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 5, 'Location de conteneur à déchets', 'equipement', 'pièce'),
    ('batiment.maconnerie', 6, 'Terrassement en déblai', 'ouvrage', 'm³'),
    ('batiment.maconnerie', 7, 'Évacuation des terres', 'ouvrage', 'm³'),
    ('batiment.maconnerie', 8, 'Semelle de fondation en béton armé', 'ouvrage', 'm³'),
    ('batiment.maconnerie', 9, 'Béton de propreté', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 10, 'Dalle de sol en béton armé', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 11, 'Chape de ciment', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 12, 'Isolation de sol sous chape', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 13, 'Membrane d''étanchéité sous dalle', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 14, 'Maçonnerie de blocs de béton', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 15, 'Maçonnerie de blocs de béton cellulaire', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 16, 'Maçonnerie de briques de parement', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 17, 'Isolation de mur creux', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 18, 'Linteau en béton ou en acier', 'ouvrage', 'pièce'),
    ('batiment.maconnerie', 19, 'Ouverture dans un mur porteur avec poutrelle', 'ouvrage', 'pièce'),
    ('batiment.maconnerie', 20, 'Étançonnement provisoire', 'ouvrage', 'forfait'),
    ('batiment.maconnerie', 21, 'Rejointoyage de façade', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 22, 'Nettoyage de façade', 'ouvrage', 'm²'),
    ('batiment.maconnerie', 23, 'Injection contre l''humidité ascensionnelle', 'ouvrage', 'm'),
    ('batiment.maconnerie', 24, 'Seuil en pierre bleue', 'ouvrage', 'm'),
    ('batiment.maconnerie', 25, 'Appui de fenêtre en pierre bleue', 'ouvrage', 'm'),
    ('batiment.maconnerie', 26, 'Égouttage : tuyau PVC enterré', 'ouvrage', 'm'),
    ('batiment.maconnerie', 27, 'Chambre de visite', 'ouvrage', 'pièce'),
    ('batiment.maconnerie', 28, 'Raccordement à l''égout public', 'ouvrage', 'forfait'),
    ('batiment.maconnerie', 29, 'Citerne d''eau de pluie', 'ouvrage', 'pièce'),
    ('batiment.maconnerie', 30, 'Ciment', 'fourniture', 'sac'),
    ('batiment.maconnerie', 31, 'Sable', 'fourniture', 'tonne'),
    ('batiment.maconnerie', 32, 'Béton prêt à l''emploi', 'fourniture', 'm³'),
    ('batiment.maconnerie', 33, 'Maçon', 'main_oeuvre', 'h'),
    ('batiment.maconnerie', 34, 'Manoeuvre', 'main_oeuvre', 'h'),
    ('batiment.maconnerie', 35, 'Location d''échafaudage', 'equipement', 'jour'),
    ('batiment.maconnerie', 36, 'Location de mini-pelle avec chauffeur', 'equipement', 'jour');

-- Gyproc et plafonnage
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('batiment.gyproc', 1, 'Cloison en plaques de plâtre sur ossature métallique', 'ouvrage', 'm²'),
    ('batiment.gyproc', 2, 'Cloison avec isolation acoustique', 'ouvrage', 'm²'),
    ('batiment.gyproc', 3, 'Doublage de mur en plaques de plâtre', 'ouvrage', 'm²'),
    ('batiment.gyproc', 4, 'Doublage de mur avec isolation', 'ouvrage', 'm²'),
    ('batiment.gyproc', 5, 'Faux plafond en plaques de plâtre', 'ouvrage', 'm²'),
    ('batiment.gyproc', 6, 'Faux plafond avec isolation', 'ouvrage', 'm²'),
    ('batiment.gyproc', 7, 'Plaque hydrofuge (pièce humide)', 'ouvrage', 'm²'),
    ('batiment.gyproc', 8, 'Plaque coupe-feu', 'ouvrage', 'm²'),
    ('batiment.gyproc', 9, 'Habillage de poutre ou de gaine', 'ouvrage', 'm'),
    ('batiment.gyproc', 10, 'Caisson technique', 'ouvrage', 'm'),
    ('batiment.gyproc', 11, 'Trappe de visite', 'ouvrage', 'pièce'),
    ('batiment.gyproc', 12, 'Joints et finition des plaques (bandes et enduit)', 'ouvrage', 'm²'),
    ('batiment.gyproc', 13, 'Cornière d''angle', 'ouvrage', 'm'),
    ('batiment.gyproc', 14, 'Plafonnage des murs (enduit au plâtre)', 'ouvrage', 'm²'),
    ('batiment.gyproc', 15, 'Plafonnage des plafonds', 'ouvrage', 'm²'),
    ('batiment.gyproc', 16, 'Enduit de lissage', 'ouvrage', 'm²'),
    ('batiment.gyproc', 17, 'Pose d''un treillis d''armature', 'ouvrage', 'm²'),
    ('batiment.gyproc', 18, 'Réparation de plafonnage existant', 'ouvrage', 'm²'),
    ('batiment.gyproc', 19, 'Isolation de toiture par l''intérieur (laine minérale)', 'ouvrage', 'm²'),
    ('batiment.gyproc', 20, 'Pare-vapeur', 'ouvrage', 'm²'),
    ('batiment.gyproc', 21, 'Plaques de plâtre', 'fourniture', 'm²'),
    ('batiment.gyproc', 22, 'Profilés métalliques', 'fourniture', 'm'),
    ('batiment.gyproc', 23, 'Plâtre à projeter', 'fourniture', 'sac'),
    ('batiment.gyproc', 24, 'Plaquiste', 'main_oeuvre', 'h'),
    ('batiment.gyproc', 25, 'Plafonneur', 'main_oeuvre', 'h'),
    ('batiment.gyproc', 26, 'Location de projeteuse à plâtre', 'equipement', 'jour');

-- Menuiserie
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('batiment.menuiserie', 1, 'Châssis en PVC, fourni et posé', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 2, 'Châssis en aluminium, fourni et posé', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 3, 'Châssis en bois, fourni et posé', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 4, 'Porte d''entrée', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 5, 'Porte de garage sectionnelle', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 6, 'Dépose et évacuation d''anciens châssis', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 7, 'Volet roulant', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 8, 'Moustiquaire', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 9, 'Fenêtre de toit', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 10, 'Porte intérieure avec chambranle', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 11, 'Porte coulissante à galandage', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 12, 'Bloc-porte coupe-feu', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 13, 'Parquet massif', 'ouvrage', 'm²'),
    ('batiment.menuiserie', 14, 'Parquet contrecollé ou stratifié', 'ouvrage', 'm²'),
    ('batiment.menuiserie', 15, 'Plinthes', 'ouvrage', 'm'),
    ('batiment.menuiserie', 16, 'Escalier en bois', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 17, 'Garde-corps', 'ouvrage', 'm'),
    ('batiment.menuiserie', 18, 'Placard sur mesure', 'ouvrage', 'm'),
    ('batiment.menuiserie', 19, 'Habillage intérieur de châssis (tablette, ébrasement)', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 20, 'Terrasse en bois', 'ouvrage', 'm²'),
    ('batiment.menuiserie', 21, 'Bardage en bois', 'ouvrage', 'm²'),
    ('batiment.menuiserie', 22, 'Réglage et entretien de châssis', 'ouvrage', 'pièce'),
    ('batiment.menuiserie', 23, 'Remplacement de vitrage', 'ouvrage', 'm²'),
    ('batiment.menuiserie', 24, 'Quincaillerie (poignées, serrures)', 'fourniture', 'pièce'),
    ('batiment.menuiserie', 25, 'Menuisier', 'main_oeuvre', 'h');

-- Électricité
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('batiment.electricite', 1, 'Tableau électrique complet', 'ouvrage', 'pièce'),
    ('batiment.electricite', 2, 'Disjoncteur', 'ouvrage', 'pièce'),
    ('batiment.electricite', 3, 'Interrupteur différentiel', 'ouvrage', 'pièce'),
    ('batiment.electricite', 4, 'Point lumineux (câblage compris)', 'ouvrage', 'pièce'),
    ('batiment.electricite', 5, 'Prise de courant simple', 'ouvrage', 'pièce'),
    ('batiment.electricite', 6, 'Prise de courant double', 'ouvrage', 'pièce'),
    ('batiment.electricite', 7, 'Interrupteur simple', 'ouvrage', 'pièce'),
    ('batiment.electricite', 8, 'Interrupteur va-et-vient', 'ouvrage', 'pièce'),
    ('batiment.electricite', 9, 'Circuit spécialisé (four, taque, lave-linge)', 'ouvrage', 'pièce'),
    ('batiment.electricite', 10, 'Saignée et rebouchage', 'ouvrage', 'm'),
    ('batiment.electricite', 11, 'Gaine et câble encastrés', 'ouvrage', 'm'),
    ('batiment.electricite', 12, 'Prise réseau (RJ45)', 'ouvrage', 'pièce'),
    ('batiment.electricite', 13, 'Prise de télévision', 'ouvrage', 'pièce'),
    ('batiment.electricite', 14, 'Spot encastré LED', 'ouvrage', 'pièce'),
    ('batiment.electricite', 15, 'Détecteur de fumée', 'ouvrage', 'pièce'),
    ('batiment.electricite', 16, 'Sonnette ou parlophone', 'ouvrage', 'pièce'),
    ('batiment.electricite', 17, 'Mise à la terre (piquet ou boucle)', 'ouvrage', 'forfait'),
    ('batiment.electricite', 18, 'Borne de recharge pour voiture', 'ouvrage', 'pièce'),
    ('batiment.electricite', 19, 'Démontage d''installation existante', 'ouvrage', 'forfait'),
    ('batiment.electricite', 20, 'Schémas unifilaire et de position', 'ouvrage', 'forfait'),
    ('batiment.electricite', 21, 'Contrôle de conformité par un organisme agréé', 'sous_traitant', 'forfait'),
    ('batiment.electricite', 22, 'Câble électrique', 'fourniture', 'm'),
    ('batiment.electricite', 23, 'Électricien', 'main_oeuvre', 'h');

-- Plomberie et chauffage
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('batiment.plomberie', 1, 'Alimentation en eau (tuyau multicouche)', 'ouvrage', 'm'),
    ('batiment.plomberie', 2, 'Décharge en PVC', 'ouvrage', 'm'),
    ('batiment.plomberie', 3, 'Raccordement d''un appareil sanitaire', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 4, 'WC suspendu avec bâti-support', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 5, 'WC sur pied', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 6, 'Lavabo ou meuble-lavabo', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 7, 'Douche à l''italienne', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 8, 'Receveur et paroi de douche', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 9, 'Baignoire', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 10, 'Robinet ou mitigeur', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 11, 'Évier de cuisine', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 12, 'Chauffe-eau électrique', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 13, 'Chaudière au gaz à condensation', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 14, 'Pompe à chaleur', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 15, 'Radiateur', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 16, 'Chauffage par le sol', 'ouvrage', 'm²'),
    ('batiment.plomberie', 17, 'Thermostat', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 18, 'Adoucisseur d''eau', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 19, 'Entretien de chaudière', 'ouvrage', 'pièce'),
    ('batiment.plomberie', 20, 'Dépose d''anciens sanitaires', 'ouvrage', 'forfait'),
    ('batiment.plomberie', 21, 'Recherche et réparation de fuite', 'ouvrage', 'forfait'),
    ('batiment.plomberie', 22, 'Débouchage', 'ouvrage', 'forfait'),
    ('batiment.plomberie', 23, 'Accessoires de raccordement', 'fourniture', 'forfait'),
    ('batiment.plomberie', 24, 'Plombier', 'main_oeuvre', 'h'),
    ('batiment.plomberie', 25, 'Chauffagiste', 'main_oeuvre', 'h'),
    ('batiment.plomberie', 26, 'Déplacement', 'main_oeuvre', 'forfait');

-- Toiture
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('batiment.toiture', 1, 'Dépose de couverture existante, évacuation comprise', 'ouvrage', 'm²'),
    ('batiment.toiture', 2, 'Charpente en bois', 'ouvrage', 'm²'),
    ('batiment.toiture', 3, 'Traitement de charpente', 'ouvrage', 'm²'),
    ('batiment.toiture', 4, 'Sous-toiture', 'ouvrage', 'm²'),
    ('batiment.toiture', 5, 'Contre-lattage et lattage', 'ouvrage', 'm²'),
    ('batiment.toiture', 6, 'Couverture en tuiles', 'ouvrage', 'm²'),
    ('batiment.toiture', 7, 'Couverture en ardoises', 'ouvrage', 'm²'),
    ('batiment.toiture', 8, 'Toiture plate : étanchéité bitumineuse', 'ouvrage', 'm²'),
    ('batiment.toiture', 9, 'Toiture plate : EPDM', 'ouvrage', 'm²'),
    ('batiment.toiture', 10, 'Isolation de toiture plate', 'ouvrage', 'm²'),
    ('batiment.toiture', 11, 'Isolation de toiture inclinée (sarking)', 'ouvrage', 'm²'),
    ('batiment.toiture', 12, 'Faîtage', 'ouvrage', 'm'),
    ('batiment.toiture', 13, 'Rive', 'ouvrage', 'm'),
    ('batiment.toiture', 14, 'Noue', 'ouvrage', 'm'),
    ('batiment.toiture', 15, 'Gouttière en zinc', 'ouvrage', 'm'),
    ('batiment.toiture', 16, 'Descente d''eau pluviale', 'ouvrage', 'm'),
    ('batiment.toiture', 17, 'Rive de toiture plate (profilé aluminium)', 'ouvrage', 'm'),
    ('batiment.toiture', 18, 'Habillage de cheminée', 'ouvrage', 'pièce'),
    ('batiment.toiture', 19, 'Lanterneau ou coupole', 'ouvrage', 'pièce'),
    ('batiment.toiture', 20, 'Planche de rive et corniche', 'ouvrage', 'm'),
    ('batiment.toiture', 21, 'Réparation ponctuelle de toiture', 'ouvrage', 'forfait'),
    ('batiment.toiture', 22, 'Nettoyage et démoussage', 'ouvrage', 'm²'),
    ('batiment.toiture', 23, 'Couvreur', 'main_oeuvre', 'h'),
    ('batiment.toiture', 24, 'Location de nacelle ou de lift', 'equipement', 'jour'),
    ('batiment.toiture', 25, 'Échafaudage de façade', 'equipement', 'm²');

-- Carrelage
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('batiment.carrelage', 1, 'Dépose d''ancien carrelage', 'ouvrage', 'm²'),
    ('batiment.carrelage', 2, 'Ragréage de sol', 'ouvrage', 'm²'),
    ('batiment.carrelage', 3, 'Primaire d''accrochage', 'ouvrage', 'm²'),
    ('batiment.carrelage', 4, 'Étanchéité sous carrelage (douche, salle de bain)', 'ouvrage', 'm²'),
    ('batiment.carrelage', 5, 'Pose de carrelage de sol', 'ouvrage', 'm²'),
    ('batiment.carrelage', 6, 'Pose de carrelage mural (faïence)', 'ouvrage', 'm²'),
    ('batiment.carrelage', 7, 'Pose de grand format', 'ouvrage', 'm²'),
    ('batiment.carrelage', 8, 'Pose de mosaïque', 'ouvrage', 'm²'),
    ('batiment.carrelage', 9, 'Plinthes carrelées', 'ouvrage', 'm'),
    ('batiment.carrelage', 10, 'Profilé de finition ou de transition', 'ouvrage', 'm'),
    ('batiment.carrelage', 11, 'Joints de carrelage', 'ouvrage', 'm²'),
    ('batiment.carrelage', 12, 'Joint silicone', 'ouvrage', 'm'),
    ('batiment.carrelage', 13, 'Carrelage extérieur sur plots', 'ouvrage', 'm²'),
    ('batiment.carrelage', 14, 'Habillage de marche d''escalier', 'ouvrage', 'pièce'),
    ('batiment.carrelage', 15, 'Carrelage (fourni par l''entreprise)', 'fourniture', 'm²'),
    ('batiment.carrelage', 16, 'Colle à carrelage', 'fourniture', 'sac'),
    ('batiment.carrelage', 17, 'Carreleur', 'main_oeuvre', 'h');

-- Entretien de jardin
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('espaces_verts.entretien', 1, 'Tonte de pelouse', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 2, 'Tonte de pelouse (passage)', 'ouvrage', 'forfait'),
    ('espaces_verts.entretien', 3, 'Taille de haie', 'ouvrage', 'm'),
    ('espaces_verts.entretien', 4, 'Taille d''arbustes', 'ouvrage', 'pièce'),
    ('espaces_verts.entretien', 5, 'Taille de rosiers', 'ouvrage', 'pièce'),
    ('espaces_verts.entretien', 6, 'Désherbage manuel des massifs', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 7, 'Scarification de pelouse', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 8, 'Aération de pelouse', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 9, 'Regarnissage de pelouse', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 10, 'Ramassage des feuilles', 'ouvrage', 'forfait'),
    ('espaces_verts.entretien', 11, 'Paillage des massifs', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 12, 'Nettoyage de terrasse ou d''allée (haute pression)', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 13, 'Débroussaillage', 'ouvrage', 'm²'),
    ('espaces_verts.entretien', 14, 'Contrat d''entretien annuel', 'ouvrage', 'forfait'),
    ('espaces_verts.entretien', 15, 'Évacuation des déchets verts', 'ouvrage', 'm³'),
    ('espaces_verts.entretien', 16, 'Frais de parc à conteneurs', 'fourniture', 'forfait'),
    ('espaces_verts.entretien', 17, 'Engrais', 'fourniture', 'kg'),
    ('espaces_verts.entretien', 18, 'Jardinier', 'main_oeuvre', 'h'),
    ('espaces_verts.entretien', 19, 'Déplacement', 'main_oeuvre', 'forfait');

-- Élagage et abattage
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('espaces_verts.elagage', 1, 'Élagage d''arbre (taille de formation ou d''entretien)', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 2, 'Taille de réduction de couronne', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 3, 'Taille en têtard', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 4, 'Retrait de bois mort', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 5, 'Haubanage d''arbre', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 6, 'Abattage direct', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 7, 'Abattage par démontage (avec cordes)', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 8, 'Abattage avec nacelle', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 9, 'Rognage de souche', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 10, 'Dessouchage', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 11, 'Broyage des branches', 'ouvrage', 'm³'),
    ('espaces_verts.elagage', 12, 'Découpe du tronc en bûches', 'ouvrage', 'stère'),
    ('espaces_verts.elagage', 13, 'Fendage du bois', 'ouvrage', 'stère'),
    ('espaces_verts.elagage', 14, 'Évacuation du bois et des déchets', 'ouvrage', 'm³'),
    ('espaces_verts.elagage', 15, 'Bois laissé sur place, rangé', 'ouvrage', 'forfait'),
    ('espaces_verts.elagage', 16, 'Diagnostic de l''arbre', 'ouvrage', 'pièce'),
    ('espaces_verts.elagage', 17, 'Demande de permis d''abattage', 'ouvrage', 'forfait'),
    ('espaces_verts.elagage', 18, 'Balisage et sécurisation de la zone', 'ouvrage', 'forfait'),
    ('espaces_verts.elagage', 19, 'Bois de chauffage', 'fourniture', 'stère'),
    ('espaces_verts.elagage', 20, 'Élagueur grimpeur', 'main_oeuvre', 'h'),
    ('espaces_verts.elagage', 21, 'Bûcheron', 'main_oeuvre', 'h'),
    ('espaces_verts.elagage', 22, 'Aide au sol', 'main_oeuvre', 'h'),
    ('espaces_verts.elagage', 23, 'Location de nacelle', 'equipement', 'jour'),
    ('espaces_verts.elagage', 24, 'Broyeur de branches', 'equipement', 'jour'),
    ('espaces_verts.elagage', 25, 'Rogneuse de souches', 'equipement', 'jour');

-- Aménagement
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('espaces_verts.amenagement', 1, 'Préparation du terrain (nivellement)', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 2, 'Apport de terre végétale', 'ouvrage', 'm³'),
    ('espaces_verts.amenagement', 3, 'Engazonnement par semis', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 4, 'Pose de gazon en rouleaux', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 5, 'Plantation de haie', 'ouvrage', 'm'),
    ('espaces_verts.amenagement', 6, 'Plantation d''arbre', 'ouvrage', 'pièce'),
    ('espaces_verts.amenagement', 7, 'Plantation d''arbustes ou de vivaces', 'ouvrage', 'pièce'),
    ('espaces_verts.amenagement', 8, 'Création de massif', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 9, 'Toile de paillage', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 10, 'Bordure (béton, acier ou bois)', 'ouvrage', 'm'),
    ('espaces_verts.amenagement', 11, 'Allée en gravier', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 12, 'Allée ou terrasse en pavés', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 13, 'Terrasse en dalles', 'ouvrage', 'm²'),
    ('espaces_verts.amenagement', 14, 'Muret de soutènement', 'ouvrage', 'm'),
    ('espaces_verts.amenagement', 15, 'Clôture (piquets et treillis)', 'ouvrage', 'm'),
    ('espaces_verts.amenagement', 16, 'Portillon', 'ouvrage', 'pièce'),
    ('espaces_verts.amenagement', 17, 'Arrosage automatique', 'ouvrage', 'forfait'),
    ('espaces_verts.amenagement', 18, 'Éclairage de jardin', 'ouvrage', 'pièce'),
    ('espaces_verts.amenagement', 19, 'Plan d''aménagement', 'ouvrage', 'forfait'),
    ('espaces_verts.amenagement', 20, 'Plantes et végétaux', 'fourniture', 'pièce'),
    ('espaces_verts.amenagement', 21, 'Gravier ou concassé', 'fourniture', 'tonne'),
    ('espaces_verts.amenagement', 22, 'Paysagiste', 'main_oeuvre', 'h'),
    ('espaces_verts.amenagement', 23, 'Location de mini-pelle', 'equipement', 'jour');

-- Peinture
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('peinture.peinture', 1, 'Protection des sols et du mobilier', 'ouvrage', 'forfait'),
    ('peinture.peinture', 2, 'Bandes de masquage', 'ouvrage', 'forfait'),
    ('peinture.peinture', 3, 'Détapissage', 'ouvrage', 'm²'),
    ('peinture.peinture', 4, 'Lessivage des murs et plafonds', 'ouvrage', 'm²'),
    ('peinture.peinture', 5, 'Ponçage', 'ouvrage', 'm²'),
    ('peinture.peinture', 6, 'Rebouchage des fissures et des trous', 'ouvrage', 'm²'),
    ('peinture.peinture', 7, 'Enduit de lissage', 'ouvrage', 'm²'),
    ('peinture.peinture', 8, 'Couche d''impression (fixateur)', 'ouvrage', 'm²'),
    ('peinture.peinture', 9, 'Peinture des murs, deux couches', 'ouvrage', 'm²'),
    ('peinture.peinture', 10, 'Peinture des plafonds, deux couches', 'ouvrage', 'm²'),
    ('peinture.peinture', 11, 'Peinture des boiseries (portes, plinthes)', 'ouvrage', 'm²'),
    ('peinture.peinture', 12, 'Peinture d''une porte, deux faces', 'ouvrage', 'pièce'),
    ('peinture.peinture', 13, 'Peinture de radiateur', 'ouvrage', 'pièce'),
    ('peinture.peinture', 14, 'Peinture de châssis', 'ouvrage', 'pièce'),
    ('peinture.peinture', 15, 'Laque', 'ouvrage', 'm²'),
    ('peinture.peinture', 16, 'Pose de papier peint', 'ouvrage', 'm²'),
    ('peinture.peinture', 17, 'Pose de toile de verre', 'ouvrage', 'm²'),
    ('peinture.peinture', 18, 'Peinture de façade', 'ouvrage', 'm²'),
    ('peinture.peinture', 19, 'Hydrofuge de façade', 'ouvrage', 'm²'),
    ('peinture.peinture', 20, 'Lasure ou vernis sur bois extérieur', 'ouvrage', 'm²'),
    ('peinture.peinture', 21, 'Peinture de sol', 'ouvrage', 'm²'),
    ('peinture.peinture', 22, 'Traitement anti-moisissures', 'ouvrage', 'm²'),
    ('peinture.peinture', 23, 'Peinture (fournie par l''entreprise)', 'fourniture', 'litre'),
    ('peinture.peinture', 24, 'Petit matériel (rouleaux, bâches)', 'fourniture', 'forfait'),
    ('peinture.peinture', 25, 'Peintre', 'main_oeuvre', 'h'),
    ('peinture.peinture', 26, 'Location d''échafaudage', 'equipement', 'jour');

-- Graphisme
INSERT INTO catalogue_ouvrages (bibliotheque, position, description, genre, unite) VALUES
    ('graphisme.graphisme', 1, 'Briefing et analyse du besoin', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 2, 'Recherches créatives et pistes graphiques', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 3, 'Direction artistique', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 4, 'Création de logo', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 5, 'Déclinaisons du logo (couleurs, formats, versions)', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 6, 'Charte graphique (guide de marque)', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 7, 'Papeterie (cartes de visite, papier à en-tête)', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 8, 'Gabarits de documents (présentation, offre)', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 9, 'Flyer ou affiche', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 10, 'Brochure ou catalogue (mise en page)', 'main_oeuvre', 'page'),
    ('graphisme.graphisme', 11, 'Rapport annuel (mise en page)', 'main_oeuvre', 'page'),
    ('graphisme.graphisme', 12, 'Kit pour les réseaux sociaux', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 13, 'Visuel pour les réseaux sociaux', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 14, 'Bannière web (display)', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 15, 'Webdesign : maquette de page', 'main_oeuvre', 'page'),
    ('graphisme.graphisme', 16, 'Prototype interactif', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 17, 'Illustration', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 18, 'Pictogrammes ou icônes', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 19, 'Packaging et étiquette', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 20, 'Signalétique', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 21, 'Covering de véhicule (création)', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 22, 'Retouche et détourage photo', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 23, 'Aller-retour de corrections supplémentaire', 'main_oeuvre', 'pièce'),
    ('graphisme.graphisme', 24, 'Exécution et fichiers pour l''imprimeur (BAT)', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 25, 'Suivi de fabrication', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 26, 'Cession des droits d''auteur', 'main_oeuvre', 'forfait'),
    ('graphisme.graphisme', 27, 'Temps de création', 'main_oeuvre', 'h'),
    ('graphisme.graphisme', 28, 'Impression', 'sous_traitant', 'pièce'),
    ('graphisme.graphisme', 29, 'Licence d''image ou de police', 'fourniture', 'pièce');

