//! Importer un devis ou un avenant depuis le modèle Excel (après l'étape 9) : envoyer le fichier, voir l'aperçu
//! (lots, lignes, total comparé à celui du fichier, alertes), puis « Importer » ajoute les lignes au brouillon
//! (à la suite, ou à la place des lignes existantes). Les prix négatifs passent tels quels (avenant en moins).
//! La lecture du fichier est dans `import_excel`.

use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Form, Multipart, Path},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use sqlx::PgConnection;

use crate::base::Base;
use crate::commun::format_euros;
use crate::devis_lignes::charger_entete;
use crate::import_excel::{self, LigneImport};
use crate::journal::{self, Genre};
use crate::{AppState, Erreur, calcul};

const TAILLE_MAX: usize = 10 * 1024 * 1024;

/// Le devis (ou l'avenant), s'il est encore un brouillon : (titre « Devis n° 2026-57 », nombre de lignes, avenant ?).
async fn brouillon(db: &mut PgConnection, id: i64) -> Result<(String, i64, bool), Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    if !entete.brouillon {
        return Err(Erreur::conflit("Ce devis n'est plus un brouillon : on ne peut plus y importer de lignes."));
    }
    let lignes: i64 = sqlx::query_scalar("SELECT count(*) FROM devis_lignes WHERE devis_id = $1").bind(id).fetch_one(&mut *db).await?;
    Ok((format!("{} n° {}", entete.genre, entete.numero), lignes, entete.genre == "Avenant"))
}

// ─── Envoyer le fichier ─────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "import_devis.html")]
struct PageEnvoi {
    id: i64,
    titre: String,
    avenant: bool,
    erreur: String,
}

async fn page_envoi(db: &mut PgConnection, id: i64, erreur: String) -> Result<Html<String>, Erreur> {
    let (titre, _, avenant) = brouillon(&mut *db, id).await?;
    Ok(Html(PageEnvoi { id, titre, avenant, erreur }.render()?))
}

async fn formulaire(mut db: Base, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    page_envoi(&mut *db, id, String::new()).await
}

async fn erreur_envoi(db: &mut PgConnection, id: i64, message: &str) -> Result<Response, Erreur> {
    Ok((StatusCode::UNPROCESSABLE_ENTITY, page_envoi(&mut *db, id, message.to_string()).await?).into_response())
}

async fn recevoir(mut db: Base, Path(id): Path<i64>, mut multipart: Multipart) -> Result<Response, Erreur> {
    brouillon(&mut *db, id).await?;
    let mut recu: Option<(String, Vec<u8>)> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(champ)) if champ.name() == Some("fichier") => {
                let nom = champ.file_name().unwrap_or("").to_string();
                match champ.bytes().await {
                    Ok(octets) => recu = Some((nom, octets.to_vec())),
                    Err(_) => return erreur_envoi(&mut *db, id, "Le fichier est trop gros (10 Mo au maximum) ou illisible.").await,
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => return erreur_envoi(&mut *db, id, "Le fichier est trop gros (10 Mo au maximum) ou illisible.").await,
        }
    }
    let Some((nom, octets)) = recu.filter(|(_, o)| !o.is_empty()) else {
        return erreur_envoi(&mut *db, id, "Choisissez un fichier Excel avant de cliquer sur « Lire le fichier ».").await;
    };
    let resultat = match tokio::task::spawn_blocking(move || import_excel::lire_xlsx(&octets)).await {
        Ok(Ok(r)) => r,
        Ok(Err(message)) => return erreur_envoi(&mut *db, id, &message).await,
        Err(_) => return erreur_envoi(&mut *db, id, "Le fichier n'a pas pu être lu.").await,
    };
    let lignes = serde_json::to_string(&resultat.lignes).map_err(|_| Erreur::conflit("Le fichier n'a pas pu être lu."))?;
    let nom: String = nom.chars().filter(|c| !c.is_control()).take(200).collect();
    // Les aperçus abandonnés de ce devis disparaissent.
    sqlx::query("DELETE FROM imports_excel WHERE devis_id = $1").bind(id).execute(&mut *db).await?;
    let import: i64 = sqlx::query_scalar(
        "INSERT INTO imports_excel (devis_id, nom_fichier, lignes, total_fichier_cents, taux_fichier, alertes)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(id)
    .bind(&nom)
    .bind(&lignes)
    .bind(resultat.total_fichier_cents)
    .bind(resultat.taux_fichier)
    .bind(resultat.alertes.join("\n"))
    .fetch_one(&mut *db)
    .await?;
    Ok(Redirect::to(&format!("/devis/{id}/import/{import}")).into_response())
}

// ─── Aperçu ─────────────────────────────────────────────────────────────────

pub struct LigneVue {
    pub numero: String,
    pub genre: &'static str,
    pub classe: String,
    pub est_titre: bool,
    pub est_poste: bool,
    pub description: String,
    pub quantite: String,
    pub unite: String,
    pub prix: String,
    pub total: String,
    pub ligne_excel: usize,
}

fn libelle(genre: &str) -> &'static str {
    match genre {
        "titre" => "Titre (lot)",
        "sous_titre" => "Sous-titre",
        "texte" => "Texte",
        _ => "Poste libre",
    }
}

/// Numérotation comme dans l'Encodage : un titre ouvre un lot (1.0), les lignes suivantes (sous-titres, textes, postes) sont 1.1, 1.2…
pub(crate) fn numeros(lignes: &[LigneImport]) -> Vec<String> {
    let (mut lot, mut rang) = (0, 0);
    lignes
        .iter()
        .map(|l| match l.genre.as_str() {
            "titre" => {
                lot += 1;
                rang = 0;
                format!("{lot}.0")
            }
            _ => {
                rang += 1;
                format!("{lot}.{rang}")
            }
        })
        .collect()
}

struct Import {
    nom_fichier: String,
    lignes: Vec<LigneImport>,
    total_fichier_cents: Option<i64>,
    taux_fichier: Option<i32>,
    alertes: Vec<String>,
}

async fn charger_import(db: &mut PgConnection, id: i64, import: i64) -> Result<Import, Erreur> {
    let ligne: Option<(String, String, Option<i64>, Option<i32>, String)> = sqlx::query_as(
        "SELECT nom_fichier, lignes, total_fichier_cents, taux_fichier, alertes FROM imports_excel WHERE id = $1 AND devis_id = $2",
    )
    .bind(import)
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let (nom_fichier, lignes, total_fichier_cents, taux_fichier, alertes) = ligne.ok_or_else(Erreur::introuvable)?;
    let lignes: Vec<LigneImport> = serde_json::from_str(&lignes).map_err(|_| Erreur::introuvable())?;
    let alertes = alertes.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    Ok(Import { nom_fichier, lignes, total_fichier_cents, taux_fichier, alertes })
}

/// Le taux de TVA des lignes importées : celui du devis, sinon celui écrit dans le fichier (s'il existe dans l'application).
async fn taux_a_utiliser(db: &mut PgConnection, id: i64, taux_fichier: Option<i32>) -> Result<Option<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT t.id, t.libelle FROM taux_tva t
         WHERE t.id = (SELECT taux_tva_id FROM devis WHERE id = $1)
            OR (NOT EXISTS (SELECT 1 FROM devis WHERE id = $1 AND taux_tva_id IS NOT NULL)
                AND t.actif AND NOT t.autoliquidation AND t.taux_centiemes = $2)
         LIMIT 1",
    )
    .bind(id)
    .bind(taux_fichier)
    .fetch_optional(&mut *db)
    .await
}

#[derive(Template)]
#[template(path = "import_apercu.html")]
struct PageApercu {
    id: i64,
    import: i64,
    titre: String,
    avenant: bool,
    nom_fichier: String,
    lignes: Vec<LigneVue>,
    nombre_lots: usize,
    nombre_postes: usize,
    total: String,
    total_fichier: String,
    totaux_egaux: bool,
    taux: String,
    taux_manquant: bool,
    alertes: Vec<String>,
    lignes_existantes: i64,
}

async fn apercu(mut db: Base, Path((id, import)): Path<(i64, i64)>) -> Result<Html<String>, Erreur> {
    let (titre, lignes_existantes, avenant) = brouillon(&mut *db, id).await?;
    let i = charger_import(&mut *db, id, import).await?;
    let total: i64 = i.lignes.iter().map(LigneImport::total_cents).sum();
    let taux = taux_a_utiliser(&mut *db, id, i.taux_fichier).await?;
    let numeros = numeros(&i.lignes);
    let page = PageApercu {
        id,
        import,
        titre,
        avenant,
        nom_fichier: i.nom_fichier,
        nombre_lots: i.lignes.iter().filter(|l| l.genre == "titre").count(),
        nombre_postes: i.lignes.iter().filter(|l| l.genre == "poste_libre").count(),
        total: format_euros(total),
        total_fichier: i.total_fichier_cents.map(format_euros).unwrap_or_default(),
        totaux_egaux: i.total_fichier_cents == Some(total),
        taux_manquant: taux.is_none(),
        taux: taux.map(|(_, l)| l).unwrap_or_default(),
        alertes: i.alertes,
        lignes_existantes,
        lignes: i
            .lignes
            .iter()
            .zip(numeros)
            .map(|(l, numero)| LigneVue {
                numero,
                genre: libelle(&l.genre),
                classe: format!("u-type-{}", l.genre),
                est_titre: l.genre == "titre",
                est_poste: l.genre == "poste_libre",
                description: l.description.clone(),
                quantite: l.quantite_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
                unite: l.unite.clone(),
                prix: l.prix_e5.map(calcul::prix_pour_affichage).unwrap_or_else(|| "—".to_string()),
                total: if l.genre == "poste_libre" { format_euros(l.total_cents()) } else { String::new() },
                ligne_excel: l.ligne_excel,
            })
            .collect(),
    };
    Ok(Html(page.render()?))
}

// ─── Importer ───────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct Confirmation {
    /// « ajouter » (à la suite) ou « remplacer » (les lignes existantes sont supprimées).
    mode: String,
}

async fn importer(mut db: Base, Path((id, import)): Path<(i64, i64)>, Form(c): Form<Confirmation>) -> Result<Redirect, Erreur> {
    brouillon(&mut *db, id).await?;
    let i = charger_import(&mut *db, id, import).await?;
    let Some((taux, _)) = taux_a_utiliser(&mut *db, id, i.taux_fichier).await? else {
        return Err(Erreur::conflit("Choisissez d'abord le taux de TVA par défaut du devis (onglet Informations), puis recommencez l'import."));
    };
    let mut tx = db.begin().await?;
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    if c.mode == "remplacer" {
        sqlx::query("DELETE FROM devis_lignes WHERE devis_id = $1").bind(id).execute(&mut *tx).await?;
    }
    let mut position: i32 =
        sqlx::query_scalar("SELECT COALESCE(max(position), 0) FROM devis_lignes WHERE devis_id = $1").bind(id).fetch_one(&mut *tx).await?;
    for l in &i.lignes {
        position += 1;
        let chiffre = l.genre == "poste_libre";
        sqlx::query(
            "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id)
             VALUES ($1, $2, $3::text::genre_ligne, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(position)
        .bind(&l.genre)
        .bind(&l.description)
        .bind(if chiffre { l.quantite_e5 } else { None })
        .bind(if chiffre { l.unite.as_str() } else { "" })
        .bind(if chiffre { Some(l.prix_e5.unwrap_or(0)) } else { None })
        .bind(chiffre.then_some(taux))
        .execute(&mut *tx)
        .await?;
    }
    crate::devis_lignes::recalculer(&mut tx, id).await?;
    let total: i64 = i.lignes.iter().map(LigneImport::total_cents).sum();
    let detail = format!(
        "{} depuis « {} »{}, total des lignes {} HT",
        crate::commun::nombre(i.lignes.len(), "ligne importée", "lignes importées"),
        i.nom_fichier,
        if c.mode == "remplacer" { " à la place des lignes existantes" } else { "" },
        format_euros(total)
    );
    journal::noter(&mut tx, id, Genre::Import, &detail).await?;
    sqlx::query("DELETE FROM imports_excel WHERE id = $1").bind(import).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/devis/{id}/encodage?import={}", i.lignes.len())))
}

/// « Nouveau devis depuis Excel » : un brouillon, puis l'envoi du fichier.
async fn nouveau(mut db: Base) -> Result<Redirect, Erreur> {
    let id = crate::devis::creer_brouillon(&mut *db).await?;
    Ok(Redirect::to(&format!("/devis/{id}/import")))
}

async fn abandonner(mut db: Base, Path((id, import)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    sqlx::query("DELETE FROM imports_excel WHERE id = $1 AND devis_id = $2").bind(import).bind(id).execute(&mut *db).await?;
    Ok(Redirect::to(&format!("/devis/{id}/encodage")))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devis/import", post(nouveau))
        .route("/devis/{id}/import", get(formulaire).post(recevoir).layer(DefaultBodyLimit::max(TAILLE_MAX + 64 * 1024)))
        .route("/devis/{id}/import/{import}", get(apercu).post(importer))
        .route("/devis/{id}/import/{import}/abandonner", post(abandonner))
        .route_layer(axum::middleware::from_fn(crate::profil::exiger_import_metres))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ligne(genre: &str) -> LigneImport {
        LigneImport { genre: genre.into(), description: String::new(), quantite_e5: None, unite: String::new(), prix_e5: None, ligne_excel: 1 }
    }

    #[test]
    fn numerotation_comme_l_encodage() {
        let l = [ligne("poste_libre"), ligne("titre"), ligne("sous_titre"), ligne("poste_libre"), ligne("texte"), ligne("poste_libre"), ligne("titre"), ligne("poste_libre")];
        assert_eq!(numeros(&l), vec!["0.1", "1.0", "1.1", "1.2", "1.3", "1.4", "2.0", "2.1"]);
    }
}
