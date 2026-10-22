// Un jeu de données complet et fictif (celui des planches) : société, entreprises et contacts, devis dans tous les
// statuts, chantier avec avancements, factures (avancement, simples, note de crédit), paiements, e-mails enregistrés,
// achats, bibliothèque, aperçu d'import Excel, export, récurrence, pages client.
// Partagé par la vitrine (`tests/vitrine.rs`) et le test d'isolation (`tests/isolation.rs`), qui l'incluent avec
// `#[path = "commun/donnees.rs"] mod donnees;`. Remplit l'organisation de la réserve et du site donnés.
// Chaque test n'utilise pas tout.
#![allow(dead_code)]

use axum::{
    Router,
    body::Body,
    http::{Request, header},
};
use sqlx::PgPool;
use tower::ServiceExt;

/// Les numéros de ce qui a été créé.
pub struct Donnees {
    pub devis: Vec<i64>,
    pub chantier: i64,
    pub avancement: i64,
    pub avancement_brouillon: i64,
    pub facture: i64,
    pub simples: Vec<i64>,
    pub import: i64,
    pub jeton_devis: String,
    pub jeton_facture: String,
}

pub async fn poster(site: &Router, uri: &str, corps: &str) -> Option<String> {
    let requete = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(corps.to_string()))
        .unwrap();
    let r = site.clone().oneshot(requete).await.unwrap();
    r.headers().get(header::LOCATION).map(|v| v.to_str().unwrap().to_string())
}

pub async fn un(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_string())).fetch_one(pool).await.unwrap()
}

pub async fn ligne(pool: &PgPool, devis: i64, position: i32, genre: &str, description: &str, q: Option<i64>, unite: &str, p: Option<i64>, achat: Option<i64>) {
    let chiffre = q.is_some();
    sqlx::query(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5, taux_tva_id)
         VALUES ($1, $2, $3::text::genre_ligne, $4, $5, $6, $7, $8, CASE WHEN $9 THEN (SELECT id FROM taux_tva WHERE taux_centiemes = 600) END)",
    )
    .bind(devis)
    .bind(position)
    .bind(genre)
    .bind(description)
    .bind(q)
    .bind(if chiffre { unite } else { "" })
    .bind(p)
    .bind(achat)
    .bind(chiffre)
    .execute(pool)
    .await
    .unwrap();
}

/// Recalcule les totaux d'un devis en passant par l'écran Encodage (une ligne « modifiée » sans changement).
pub async fn recalculer(pool: &PgPool, devis: i64) {
    let total: i64 = un(
        pool,
        &format!(
            "SELECT COALESCE(sum(round(quantite_e5::numeric * prix_unitaire_e5 / 100000000)), 0)::bigint FROM devis_lignes WHERE devis_id = {devis} AND quantite_e5 IS NOT NULL"
        ),
    )
    .await;
    sqlx::query("UPDATE devis SET montant_ht_cents = $1, montant_ttc_cents = $1 + round($1 * 0.06)::bigint WHERE id = $2")
        .bind(total)
        .bind(devis)
        .execute(pool)
        .await
        .unwrap();
}


/// Remplit l'organisation (celle de `pool` et de `s`) avec toutes les données de la vitrine.
pub async fn remplir(pool: &PgPool, s: &Router) -> Donnees {
    sqlx::query(
        "INSERT INTO societe (nom, nom_abrege, rue, numero, code_postal, ville, email, telephone, numero_tva)
         VALUES ('Ubique SRL', 'Ubique', 'Rue de l''Exemple', '1', '1300', 'Wavre', 'info@exemple.be', '010 00 00 00', 'BE0123456789')",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO societe_comptes_bancaires (iban, par_defaut) VALUES ('BE68 5390 0754 7034', true)").execute(pool).await.unwrap();

    // Contacts et entreprises fictifs (ceux des planches).
    for (nom, rue, cp, ville, tel) in [
        ("Hôtel du Parc SA", "Boulevard du Parc 1", "1000", "Bruxelles", "02 123 45 67"),
        ("Résidence Les Tilleuls", "Avenue des Tilleuls 4", "5000", "Namur", "081 22 33 44"),
        ("Peeters Construct SRL", "Industrieweg 5", "3000", "Louvain", "016 11 22 33"),
        ("ASBL Le Préau", "Rue du Préau 9", "1348", "Louvain-la-Neuve", "010 45 45 45"),
    ] {
        sqlx::query("INSERT INTO entreprises (nom, profils, rue, code_postal, ville, telephone, numero_tva) VALUES ($1, '{client}', $2, $3, $4, $5, 'BE0987654321')")
            .bind(nom)
            .bind(rue)
            .bind(cp)
            .bind(ville)
            .bind(tel)
            .execute(pool)
            .await
            .unwrap();
    }
    for (nom, prenom, profil, rue, cp, ville, tel, mail) in [
        ("Lambert", "Marc", "client", "Rue des Écoles 12", "1370", "Jodoigne", "0470 12 34 56", "lambert@exemple.be"),
        ("Claes", "Anne", "client", "Rue Haute 21", "1300", "Wavre", "0486 55 44 33", "a.claes@exemple.be"),
        ("Dubois", "Annick", "client", "Place du Marché 2", "5030", "Gembloux", "0499 88 77 66", "annick.dubois@exemple.be"),
        ("Janssens", "Pieter", "prospect", "Rue de la Station 8", "1380", "Lasne", "0471 23 45 67", ""),
        ("Renard", "Paul", "architecte", "Rue Royale 100", "1000", "Bruxelles", "02 640 10 20", "p.renard@exemple.be"),
    ] {
        sqlx::query(
            "INSERT INTO contacts (nom, prenom, profils, rue, code_postal, ville, telephone, email) VALUES ($1, $2, ARRAY[$3], $4, $5, $6, $7, NULLIF($8, ''))",
        )
        .bind(nom)
        .bind(prenom)
        .bind(profil)
        .bind(rue)
        .bind(cp)
        .bind(ville)
        .bind(tel)
        .bind(mail)
        .execute(pool)
        .await
        .unwrap();
    }
    let lambert = un(pool, "SELECT id FROM contacts WHERE nom = 'Lambert'").await;
    let conditions = un(pool, "SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = 7").await;
    let tva6 = un(pool, "SELECT id FROM taux_tva WHERE taux_centiemes = 600").await;

    // Des devis dans tous les statuts.
    let mut devis_ids = Vec::new();
    for (numero, date, reference, statut, client) in [
        (50, "2026-08-20", "L63 Toiture", "envoye", "c:Lambert"),
        (55, "2026-10-08", "C44 Volets", "envoye", "e:Résidence Les Tilleuls"),
        (56, "2026-10-12", "PL46 Salle de bain", "accepte", "c:Claes"),
        (57, "2026-10-14", "F28 Joues de lucarnes", "envoye", "c:Lambert"),
        (58, "2026-10-15", "HP2 Chambres 3e étage", "a_envoyer", "e:Hôtel du Parc SA"),
        (54, "2026-10-06", "C44 Climatisation", "refuse", "e:Résidence Les Tilleuls"),
        (59, "2026-10-15", "BS14 Toiture plate", "brouillon", "c:Janssens"),
    ] {
        let (genre, nom) = client.split_once(':').unwrap();
        let (e, c): (Option<i64>, Option<i64>) = if genre == "e" {
            (Some(un(pool, &format!("SELECT id FROM entreprises WHERE nom = '{}'", nom.replace('\'', "''"))).await), None)
        } else {
            (None, Some(un(pool, &format!("SELECT id FROM contacts WHERE nom = '{nom}'")).await))
        };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO devis (numero, date_devis, reference, responsable, statut, client_entreprise_id, client_contact_id, taux_tva_id,
                                conditions_paiement_id, date_refus, motif_refus)
             VALUES ($1, $2::date, $3, 'Max', $4::text::statut_devis, $5, $6, $7, $8,
                     CASE WHEN $4 = 'refuse' THEN $2::date + 4 END, CASE WHEN $4 = 'refuse' THEN 'Autre entreprise choisie' ELSE '' END)
             RETURNING id",
        )
        .bind(numero)
        .bind(date)
        .bind(reference)
        .bind(statut)
        .bind(e)
        .bind(c)
        .bind(tva6)
        .bind(conditions)
        .fetch_one(pool)
        .await
        .unwrap();
        devis_ids.push(id);
        ligne(pool, id, 1, "titre", "Toiture", None, "", None, None).await;
        ligne(pool, id, 2, "main_oeuvre", "Démontage de l'ancienne couverture et évacuation", Some(2_400_000), "h", Some(4_500_000), Some(2_800_000)).await;
        ligne(pool, id, 3, "fourniture", "Panneaux isolants PIR 12 cm", Some(3_800_000), "m²", Some(4_250_000), Some(2_900_000)).await;
        ligne(pool, id, 4, "texte", "Garantie de 10 ans sur l'étanchéité.", None, "", None, None).await;
        ligne(pool, id, 5, "titre", "Évacuations et chantier", None, "", None, None).await;
        ligne(pool, id, 6, "sous_traitant", "Location d'une nacelle", Some(300_000), "j", Some(28_000_000), Some(22_000_000)).await;
        recalculer(pool, id).await;
    }
    sqlx::query("UPDATE devis SET statut_date = date_devis + interval '1 day 10 hours'").execute(pool).await.unwrap();
    sqlx::query("UPDATE compteurs SET valeur = 59, annee = EXTRACT(YEAR FROM now() AT TIME ZONE 'Europe/Brussels')::int WHERE nom = 'devis'").execute(pool).await.unwrap();
    sqlx::query("UPDATE devis SET marge_activee = true, frais_generaux_centiemes = 800 WHERE numero = 59").execute(pool).await.unwrap();
    // Métré de l'isolation du devis n° 59 (étape 4-2) : 6,25 × 3,2 + 5,8 × 3,2 − 0,7 × 0,8 = 38 m², la quantité de la ligne.
    sqlx::query(
        "WITH l AS (UPDATE devis_lignes SET type_quantite = 'estimee'
                    WHERE position = 3 AND devis_id = (SELECT id FROM devis WHERE numero = 59 AND chantier_id IS NULL) RETURNING id)
         INSERT INTO devis_metres (ligne_id, position, libelle, longueur_e5, largeur_e5, deduction)
         SELECT l.id, m.position, m.libelle, m.longueur, m.largeur, m.deduction FROM l,
                (VALUES (1, 'Versant nord', 625000::bigint, 320000::bigint, false), (2, 'Versant sud', 580000, 320000, false),
                        (3, 'Fenêtre de toit', 70000, 80000, true)) AS m (position, libelle, longueur, largeur, deduction)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE devis SET client_contact_id = $1 WHERE numero = 50").bind(lambert).execute(pool).await.unwrap();

    // Le devis 50 est accepté : chantier, avancement facturé, puis un avancement en brouillon.
    let d50 = devis_ids[0];
    poster(s, &format!("/devis/{d50}/statut/accepter"), "date=2026-08-28").await;
    let d56 = devis_ids[2];
    sqlx::query("UPDATE devis SET statut = 'envoye' WHERE id = $1").bind(d56).execute(pool).await.unwrap();
    poster(s, &format!("/devis/{d56}/statut/accepter"), "date=2026-10-15").await;
    let chantier = un(pool, &format!("SELECT id FROM chantiers WHERE devis_id = {d50}")).await;
    let lignes: Vec<i64> = sqlx::query_scalar("SELECT id FROM devis_lignes WHERE devis_id = $1 AND quantite_e5 IS NOT NULL ORDER BY position")
        .bind(d50)
        .fetch_all(pool)
        .await
        .unwrap();
    let lieu = poster(s, &format!("/chantiers/{chantier}/avancements"), "").await.unwrap();
    let av: i64 = lieu.rsplit('/').next().unwrap().split('?').next().unwrap().parse().unwrap();
    poster(s, &format!("/chantiers/{chantier}/avancements/{av}/infos"), "date=2026-09-15&reference=Septembre").await;
    poster(s, &format!("/chantiers/{chantier}/avancements/{av}/lignes/l{}", lignes[0]), "pourcentage=100").await;
    poster(s, &format!("/chantiers/{chantier}/avancements/{av}/lignes/l{}", lignes[1]), "pourcentage=50").await;
    poster(s, &format!("/chantiers/{chantier}/avancements/{av}/valider"), "").await;
    let lieu = poster(s, &format!("/chantiers/{chantier}/avancements/{av}/facturer"), "").await.unwrap();
    let facture: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    poster(s, &format!("/factures/{facture}/valider"), "").await;
    poster(s, &format!("/factures/{facture}/envoyee"), "").await;
    let lieu = poster(s, &format!("/chantiers/{chantier}/avancements"), "").await.unwrap();
    let av2: i64 = lieu.rsplit('/').next().unwrap().split('?').next().unwrap().parse().unwrap();
    poster(s, &format!("/chantiers/{chantier}/avancements/{av2}/infos"), "date=2026-10-16&reference=Octobre").await;
    poster(s, &format!("/chantiers/{chantier}/avancements/{av2}/lignes/l{}", lignes[1]), "pourcentage=20").await;

    // Une facture simple validée et créditée, et une facture simple en brouillon.
    let lieu = poster(s, "/factures", "").await.unwrap();
    let simple: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
    let aujourdhui: String = sqlx::query_scalar("SELECT ((now() AT TIME ZONE 'Europe/Brussels')::date)::text").fetch_one(pool).await.unwrap();
    let claes = un(pool, "SELECT id FROM contacts WHERE nom = 'Claes'").await;
    poster(s, &format!("/factures/{simple}"), &format!("date={aujourdhui}&reference=L63+Intervention&responsable=Max&client=c{claes}&conditions_paiement_id={conditions}")).await;
    poster(s, &format!("/factures/{simple}/lignes"), "genre=poste").await;
    let l: i64 = un(pool, &format!("SELECT max(id) FROM facture_lignes WHERE facture_id = {simple}")).await;
    poster(s, &format!("/factures/{simple}/lignes/{l}"), &format!("description=D%C3%A9pannage+fuite&quantite=3&unite=h&prix_unitaire=55&taux_tva_id={tva6}")).await;
    poster(s, &format!("/factures/{simple}/valider"), "").await;
    poster(s, &format!("/factures/{simple}/crediter"), "").await;
    let lieu = poster(s, "/factures", "").await.unwrap();
    let _brouillon = lieu;

    // Étape 8 : paiements. La facture d'avancement est payée en partie ; deux factures simples de plus :
    // l'une en retard (échéance forcée dans le passé), l'autre payée.
    let virement = un(pool, "SELECT id FROM methodes_paiement WHERE libelle = 'Virement'").await;
    poster(s, &format!("/factures/{facture}/paiements"), &format!("genre=partiel&date={aujourdhui}&montant=1000&methode_id={virement}")).await;
    let dubois = un(pool, "SELECT id FROM contacts WHERE nom = 'Dubois'").await;
    let hotel = un(pool, "SELECT id FROM entreprises WHERE nom = 'Hôtel du Parc SA'").await;
    let mut simples = Vec::new();
    for (client, reference, description, quantite, prix) in
        [(format!("e{hotel}"), "HP1 Dépannage chauffe-eau", "Remplacement du chauffe-eau", "1", "3224,30"), (format!("c{dubois}"), "PD37 Dégâts des eaux", "Réparation du plafond", "12", "45")]
    {
        let lieu = poster(s, "/factures", "").await.unwrap();
        let id: i64 = lieu.strip_prefix("/factures/").unwrap().split('?').next().unwrap().parse().unwrap();
        poster(s, &format!("/factures/{id}"), &format!("date={aujourdhui}&reference={reference}&responsable=Max&client={client}&conditions_paiement_id={conditions}")).await;
        poster(s, &format!("/factures/{id}/lignes"), "genre=poste").await;
        let l: i64 = un(pool, &format!("SELECT max(id) FROM facture_lignes WHERE facture_id = {id}")).await;
        poster(s, &format!("/factures/{id}/lignes/{l}"), &format!("description={description}&quantite={quantite}&unite=forfait&prix_unitaire={prix}&taux_tva_id={tva6}")).await;
        poster(s, &format!("/factures/{id}/valider"), "").await;
        poster(s, &format!("/factures/{id}/envoyee"), "").await;
        simples.push(id);
    }
    sqlx::query("UPDATE factures SET date_echeance = date_echeance - 31 WHERE id = $1").bind(simples[0]).execute(pool).await.unwrap();
    poster(s, &format!("/factures/{}/paiements", simples[1]), &format!("genre=complet&date={aujourdhui}&methode_id={virement}")).await;

    // Étape 8-2 : deux e-mails enregistrés (mode test, rien ne part). 8-3 : la facture en retard est à relancer.
    sqlx::query("UPDATE entreprises SET email = 'compta@hotel-du-parc.example' WHERE id = $1").bind(hotel).execute(pool).await.unwrap();
    poster(s, &format!("/factures/{facture}/email"), "destinataires=lambert%40exemple.be&objet=Facture+n%C2%B0+125&corps=Bonjour").await;
    poster(s, &format!("/devis/{}/email", devis_ids[3]), "destinataires=lambert%40exemple.be&objet=Devis+F28&corps=Bonjour").await;
    // Étape 9-2 : deux achats saisis sur le chantier.
    poster(s, &format!("/chantiers/{chantier}/achats"), &format!("date={aujourdhui}&categorie=fourniture&fournisseur=Brico+Pro&description=Panneaux+PIR&montant=950")).await;
    poster(s, &format!("/chantiers/{chantier}/achats"), &format!("date={aujourdhui}&categorie=sous_traitant&fournisseur=Nacelles+SA&description=Location+3+jours&montant=640")).await;
    // Étape 9-3 : quelques ouvrages dans la bibliothèque.
    for corps in [
        "description=Panneaux+isolants+PIR+12+cm&genre=fourniture&categorie=Toiture&fournisseur=Brico+Pro&unite=m%C2%B2&prix_achat=29&prix_vente=42,50",
        "description=D%C3%A9montage+de+l%27ancienne+couverture&genre=main_oeuvre&categorie=Toiture&unite=h&prix_achat=28&marge=60",
        "description=Location+d%27une+nacelle&genre=sous_traitant&categorie=Chantier&fournisseur=Nacelles+SA&unite=j&prix_achat=220&prix_vente=280",
        "description=Remplacement+d%27un+chauffe-eau+80+l&genre=ouvrage&categorie=Sanitaire&unite=forfait&prix_achat=640&marge=35",
        "description=Pose+de+goutti%C3%A8res+en+zinc&genre=ouvrage&unite=m&prix_vente=58",
    ] {
        poster(s, "/bibliotheque", corps).await;
    }
    // Import Excel : un aperçu prêt pour le devis brouillon (lignes lues depuis un tableau sur le modèle).
    let import: i64 = {
        use ubifacture::import_excel::{Cellule::{self, Nombre as N, Texte}, lire_grille};
        let t = |s: &str| Texte(s.to_string());
        let v = Cellule::Vide;
        let grille = vec![
            vec![t("POSTE"), v.clone(), t("DESCRIPTION"), v.clone(), v.clone(), v.clone(), v.clone(), t("Qté"), t("UNITE"), t("PRIX"), t("PRIX")],
            vec![v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), t("PREVUE"), v.clone(), t("UNITAIRE"), t("H.T.V.A.")],
            vec![t("LOT 0"), v.clone(), t("ENTREPRISE - CHANTIER")],
            vec![v.clone(), v.clone(), t("INSTALLATION DE CHANTIER")],
            vec![v.clone(), v.clone(), t("Évacuation des déchets inertes"), v.clone(), v.clone(), v.clone(), v.clone(), N(1.0), t("QP"), N(140.0), N(140.0)],
            vec![v.clone(), v.clone(), t("Déplacement"), v.clone(), v.clone(), v.clone(), v.clone(), N(4.0), t("QP"), N(50.0), N(200.0)],
            vec![t("LOT 1"), v.clone(), t("PARACHEVEMENT INTERIEUR")],
            vec![v.clone(), t("1.1"), t("ENDUITS INTERIEURS")],
            vec![v.clone(), v.clone(), t("Les enduits comprennent la préparation du support.")],
            vec![v.clone(), v.clone(), t("Enduisage et réparation des murs"), v.clone(), v.clone(), v.clone(), v.clone(), N(1.0), t("fft"), N(540.0), N(540.0)],
            vec![v.clone(), t("option"), t("Plafonnage des pignons"), v.clone(), v.clone(), v.clone(), v.clone(), N(1.0), t("fft"), N(3570.0), N(3570.0)],
            vec![v.clone(), v.clone(), t("Chape pour passages techniques")],
            vec![v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), N(60.0), t("M2"), N(47.0), N(2820.0)],
            vec![v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), t("TOTAL H.T.V.A."), v.clone(), N(7270.0)],
            vec![v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), v.clone(), t("T.V.A. 6%"), v.clone(), N(436.2)],
        ];
        let r = lire_grille(&grille).unwrap();
        let import: i64 = sqlx::query_scalar(
            "INSERT INTO imports_excel (devis_id, nom_fichier, lignes, total_fichier_cents, taux_fichier, alertes) VALUES ($1, 'Devis BS14.xlsx', $2, $3, $4, '') RETURNING id",
        )
        .bind(devis_ids[6])
        .bind(serde_json::to_string(&r.lignes).unwrap())
        .bind(r.total_fichier_cents)
        .bind(r.taux_fichier)
        .fetch_one(pool)
        .await
        .unwrap();
        import
    };
    // Étape 5-3 : une facture fournisseur reçue par Peppol (fournisseur créé à l'arrivée), rattachée en partie au
    // chantier ; une seconde encore à traiter.
    let fournisseur: i64 = sqlx::query_scalar(
        "INSERT INTO entreprises (nom, profils, numero_tva, rue, code_postal, ville) VALUES ('Négoce Matériaux Wavre', '{fournisseur_autres}', 'BE0987654394', 'Rue du Dépôt 4', '1300', 'Wavre') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    for (document, numero, ht) in [("doc_vitrine_1", "NMW-2026-0412", 84_250_i64), ("doc_vitrine_2", "NMW-2026-0431", 12_900)] {
        let recue: i64 = sqlx::query_scalar(
            "INSERT INTO factures_recues (document_id, genre, expediteur, fournisseur_id, fournisseur_nom, fournisseur_tva, numero, date_facture,
                 date_echeance, montant_ht_cents, montant_tva_cents, montant_ttc_cents, a_payer_cents, iban, communication, validation, donnees,
                 xml, empreinte, pdf, recue_le)
             VALUES ($1, 'facture', '0208:0987654394', $2, 'Négoce Matériaux Wavre', 'BE0987654394', $3, $4::date, $4::date + 30, $5, $5 * 21 / 100,
                 $5 + $5 * 21 / 100, $5 + $5 * 21 / 100, 'BE68539007547034', '000041200048', 'valid',
                 jsonb_build_object('lines', jsonb_build_array(jsonb_build_object('name', 'Blocs béton 14 cm, palette', 'quantity', '5.00', 'unitCode', 'C62',
                     'netPriceAmount', '1.00', 'netAmount', ($5 / 100.0)::text, 'vat', jsonb_build_object('percentage', '21.00')))),
                 '<Invoice><ID>' || $3 || '</ID></Invoice>', repeat('a', 64), '%PDF-1.4 facture fournisseur'::bytea, now())
             RETURNING id",
        )
        .bind(document)
        .bind(fournisseur)
        .bind(numero)
        .bind(&aujourdhui)
        .bind(ht)
        .fetch_one(pool)
        .await
        .unwrap();
        if document == "doc_vitrine_1" {
            poster(s, &format!("/achats/recues/{recue}/rattacher"), &format!("destination={chantier}&montant=600&categorie=fourniture")).await;
        }
    }

    // Étape 8-4 : un dossier d'export du jour, et l'adresse du comptable.
    poster(s, "/exports/comptable", "email=compta%40fiduciaire.example").await;
    poster(s, "/exports", &format!("debut={aujourdhui}&fin={aujourdhui}&format=les_deux&contenu=toutes")).await;

    // Une facture récurrente (location mensuelle).
    poster(s, &format!("/factures/{}/recurrence", simples[1]), &format!("date_debut={aujourdhui}&date_fin=2027-12-31&frequence=mois&intervalle=1&jours_avance=0")).await;
    // Les pages client : le devis F28 (envoyé) et la facture d'avancement (liens fixes pour la vitrine).
    let jeton_devis: String = sqlx::query_scalar("UPDATE devis SET jeton = COALESCE(jeton, 'vitrine-devis-f28') WHERE id = $1 RETURNING jeton").bind(devis_ids[3]).fetch_one(pool).await.unwrap();
    let jeton_facture: String = sqlx::query_scalar("UPDATE factures SET jeton = COALESCE(jeton, 'vitrine-facture-125') WHERE id = $1 RETURNING jeton").bind(facture).fetch_one(pool).await.unwrap();
    Donnees { devis: devis_ids, chantier, avancement: av, avancement_brouillon: av2, facture, simples, import, jeton_devis, jeton_facture }
}
