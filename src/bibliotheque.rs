//! Bibliothèque de prix (étape 9-3), comme celle de Vertuoza : liste des ouvrages avec recherche et catégories,
//! fiche (description, type, catégorie, fournisseur, unité, prix d'achat, marge, prix de vente, remarque), et
//! insertion dans l'Encodage d'un devis (route dans `devis_lignes`).
//!
//! La marge se saisit ou se déduit : avec un prix de vente, la marge est recalculée ; sans prix de vente, il est
//! calculé depuis le prix d'achat et la marge (prix de vente = achat × (1 + marge)).

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query},
    http::HeaderMap,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use sqlx::{FromRow, PgConnection};

use crate::base::Base;
use crate::calcul;
use crate::commun::{Choix, echapper_like, encoder_url};
use crate::{AppState, Erreur};

const PAR_PAGE: i64 = 20;
const LONGUEUR_MAX_DESCRIPTION: usize = 5_000;
const LONGUEUR_MAX_COURT: usize = 100;
const LONGUEUR_MAX_UNITE: usize = 20;

/// Les types de ligne chiffrés, comme dans l'Encodage du devis.
pub(crate) const GENRES: [(&str, &str); 6] = [
    ("ouvrage", "Ouvrage"),
    ("fourniture", "Fourniture"),
    ("main_oeuvre", "Main d'œuvre"),
    ("sous_traitant", "Sous-traitant"),
    ("equipement", "Équipements"),
    ("poste_libre", "Poste libre"),
];

fn libelle_genre(code: &str) -> &'static str {
    crate::profil::type_ligne(GENRES.iter().find(|(c, _)| *c == code).map(|(_, l)| *l).unwrap_or("Ouvrage"))
}

/// Ce qu'affiche un prix de vente vide.
pub(crate) const PRIX_A_COMPLETER: &str = "À compléter";

/// « 42,5 » € → « 42,50 € ».
fn prix(e5: i64) -> String {
    format!("{}\u{a0}€", calcul::prix_pour_affichage(e5))
}

/// La marge en pour cent du prix d'achat, en 1/100 000 (« 46,55 % » = 4 655 000). `None` sans prix d'achat.
pub(crate) fn marge_e5(achat_e5: i64, vente_e5: i64) -> Option<i64> {
    (achat_e5 > 0).then(|| {
        let n = i128::from(vente_e5 - achat_e5) * 10_000_000;
        let d = i128::from(achat_e5);
        let q = n / d;
        // Arrondi au plus proche.
        let r = n % d;
        (if 2 * r.abs() >= d { q + n.signum() } else { q }) as i64
    })
}

/// Le prix de vente depuis le prix d'achat et la marge (en %, 1/100 000), arrondi au 1/100 000 d'euro.
pub(crate) fn vente_depuis_marge(achat_e5: i64, marge_e5: i64) -> i64 {
    let n = i128::from(achat_e5) * (10_000_000 + i128::from(marge_e5));
    let d: i128 = 10_000_000;
    ((n + d / 2).div_euclid(d)) as i64
}

fn marge_texte(achat: Option<i64>, vente: Option<i64>) -> String {
    match achat.zip(vente).and_then(|(a, v)| marge_e5(a, v)) {
        Some(m) => {
            let centiemes = (m + if m >= 0 { 500 } else { -500 }) / 1_000;
            calcul::taux_en_texte(centiemes as i32)
        }
        None => "—".to_string(),
    }
}

// ─── Liste ──────────────────────────────────────────────────────────────────

#[derive(FromRow)]
struct OuvrageLu {
    id: i64,
    description: String,
    genre: String,
    categorie: String,
    fournisseur: String,
    unite: String,
    pub(crate) prix_achat_e5: Option<i64>,
    pub(crate) prix_vente_e5: Option<i64>,
}

pub struct Ligne {
    pub id: i64,
    pub description: String,
    pub genre: &'static str,
    pub classe_genre: String,
    pub categorie: String,
    pub fournisseur: String,
    pub unite: String,
    pub achat: String,
    pub marge: String,
    pub vente: String,
    /// Le prix de vente est vide : « À compléter ».
    pub sans_prix: bool,
}

impl From<OuvrageLu> for Ligne {
    fn from(b: OuvrageLu) -> Self {
        Ligne {
            id: b.id,
            genre: libelle_genre(&b.genre),
            classe_genre: format!("u-type-{}", b.genre),
            achat: b.prix_achat_e5.map(prix).unwrap_or_else(|| "—".to_string()),
            marge: marge_texte(b.prix_achat_e5, b.prix_vente_e5),
            vente: b.prix_vente_e5.map(prix).unwrap_or_else(|| PRIX_A_COMPLETER.to_string()),
            sans_prix: b.prix_vente_e5.is_none(),
            description: b.description,
            categorie: b.categorie,
            fournisseur: b.fournisseur,
            unite: b.unite,
        }
    }
}

pub struct Categorie {
    pub nom: String,
    pub libelle: String,
    pub nombre: i64,
    pub choisie: bool,
    pub adresse: String,
}

pub struct Liste {
    pub lignes: Vec<Ligne>,
    pub q: String,
    pub categorie: String,
    pub categories: Vec<Categorie>,
    pub requete: String,
    pub page: i64,
    pub pages: i64,
    pub total: i64,
}

#[derive(Template)]
#[template(path = "bibliotheque_liste.html")]
struct PageListe {
    liste: Liste,
    message_ok: &'static str,
    message_erreur: &'static str,
    /// Le nombre d'ouvrages de la bibliothèque (toutes catégories), pour « Vider ma bibliothèque ».
    tout: i64,
}

#[derive(Template)]
#[template(path = "_bibliotheque_tableau.html")]
struct TableauListe {
    liste: Liste,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Recherche {
    q: String,
    /// Le nom d'une catégorie, « - » pour « Sans catégorie », vide pour toutes.
    categorie: String,
    page: String,
    ok: String,
    erreur: String,
}

const DEPUIS: &str = "FROM ouvrages WHERE ($1 = '' OR description ILIKE $2 OR categorie ILIKE $2 OR fournisseur ILIKE $2)
                       AND ($3 = '' OR ($3 = '-' AND categorie = '') OR categorie = $3)";

async fn liste(mut db: Base, Query(r): Query<Recherche>, entetes: HeaderMap) -> Result<Html<String>, Erreur> {
    let q = r.q.trim().to_string();
    let motif = format!("%{}%", echapper_like(&q));
    let categorie = r.categorie.trim().to_string();
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {DEPUIS}")))
        .bind(&q)
        .bind(&motif)
        .bind(&categorie)
        .fetch_one(&mut *db)
        .await?;
    let pages = ((total + PAR_PAGE - 1) / PAR_PAGE).max(1);
    let page: i64 = r.page.trim().parse().unwrap_or(1).clamp(1, pages);
    let lignes: Vec<OuvrageLu> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, description, genre::text AS genre, categorie, fournisseur, unite, prix_achat_e5, prix_vente_e5
         {DEPUIS} ORDER BY lower(categorie), lower(description), id LIMIT $4 OFFSET $5"
    )))
    .bind(&q)
    .bind(&motif)
    .bind(&categorie)
    .bind(PAR_PAGE)
    .bind((page - 1) * PAR_PAGE)
    .fetch_all(&mut *db)
    .await?;
    let groupes: Vec<(String, i64)> =
        sqlx::query_as("SELECT categorie, count(*) FROM ouvrages GROUP BY categorie ORDER BY categorie = '', lower(categorie)").fetch_all(&mut *db).await?;
    let tout: i64 = groupes.iter().map(|(_, n)| n).sum();
    let mut categories = vec![Categorie {
        nom: String::new(),
        libelle: "Toutes".to_string(),
        nombre: tout,
        choisie: categorie.is_empty(),
        adresse: format!("/bibliotheque?q={}", encoder_url(&q)),
    }];
    categories.extend(groupes.into_iter().map(|(nom, nombre)| {
        let code = if nom.is_empty() { "-".to_string() } else { nom.clone() };
        Categorie {
            libelle: if nom.is_empty() { "Sans catégorie".to_string() } else { nom.clone() },
            choisie: categorie == code,
            adresse: format!("/bibliotheque?q={}&categorie={}", encoder_url(&q), encoder_url(&code)),
            nom,
            nombre,
        }
    }));
    let liste = Liste {
        lignes: lignes.into_iter().map(Ligne::from).collect(),
        requete: format!("q={}&categorie={}", encoder_url(&q), encoder_url(&categorie)),
        q,
        categorie,
        categories,
        page,
        pages,
        total,
    };
    let partiel = entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request");
    let html = if partiel {
        TableauListe { liste }.render()?
    } else {
        let message_ok = match r.ok.as_str() {
            "cree" => "L'ouvrage est ajouté à la bibliothèque.",
            "modifie" => "L'ouvrage est enregistré.",
            "supprime" => "L'ouvrage est retiré de la bibliothèque (les devis qui l'utilisent ne changent pas).",
            "vide" => "Votre bibliothèque est vide (les devis ne changent pas). Cochez des bibliothèques dans Réglages › Bibliothèques, ou ajoutez vos ouvrages.",
            _ => "",
        };
        let message_erreur = if r.erreur == "droits" { "Seul un propriétaire peut vider la bibliothèque." } else { "" };
        PageListe { liste, message_ok, message_erreur, tout }.render()?
    };
    Ok(Html(html))
}

// ─── Fiche ──────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct Saisie {
    pub(crate) description: String,
    pub(crate) genre: String,
    pub(crate) categorie: String,
    pub(crate) fournisseur: String,
    pub(crate) unite: String,
    pub(crate) prix_achat: String,
    pub(crate) marge: String,
    pub(crate) prix_vente: String,
    pub(crate) remarque: String,
}

#[derive(Debug)]
pub(crate) struct Valeurs {
    pub(crate) description: String,
    pub(crate) genre: &'static str,
    pub(crate) categorie: String,
    pub(crate) fournisseur: String,
    pub(crate) unite: String,
    pub(crate) prix_achat_e5: Option<i64>,
    /// Vide : « Prix à compléter ».
    pub(crate) prix_vente_e5: Option<i64>,
    pub(crate) remarque: String,
}

/// Vérifie la saisie ; renvoie toutes les erreurs d'un coup.
pub(crate) fn verifier(s: &Saisie) -> Result<Valeurs, Vec<String>> {
    let mut erreurs = Vec::new();
    let description = s.description.trim().to_string();
    if description.is_empty() || description.chars().count() > LONGUEUR_MAX_DESCRIPTION {
        erreurs.push("La description est obligatoire (5 000 caractères au maximum).".to_string());
    }
    let genre = GENRES.iter().map(|(c, _)| *c).find(|c| *c == s.genre.trim());
    if genre.is_none() {
        erreurs.push("Choisissez le type.".to_string());
    }
    let court = |t: &str| t.trim().chars().count() <= LONGUEUR_MAX_COURT;
    if !court(&s.categorie) || !court(&s.fournisseur) {
        erreurs.push("Catégorie et fournisseur : 100 caractères au maximum.".to_string());
    }
    if s.unite.trim().chars().count() > LONGUEUR_MAX_UNITE {
        erreurs.push("L'unité est trop longue (20 caractères au maximum).".to_string());
    }
    let nombre = |t: &str| -> Result<Option<i64>, ()> {
        match t.trim() {
            "" => Ok(None),
            t => calcul::lire_decimal_e5(t).map(Some).ok_or(()),
        }
    };
    let achat = match nombre(&s.prix_achat) {
        Ok(Some(a)) if a < 0 => {
            erreurs.push("Le prix d'achat ne peut pas être négatif.".to_string());
            None
        }
        Ok(a) => a,
        Err(()) => {
            erreurs.push("Le prix d'achat n'est pas un nombre.".to_string());
            None
        }
    };
    let marge = nombre(&s.marge).unwrap_or_else(|()| {
        erreurs.push("La marge n'est pas un nombre.".to_string());
        None
    });
    // Ni prix de vente, ni prix d'achat et marge : le prix reste vide (« à compléter »).
    let vente = match (nombre(&s.prix_vente), achat, marge) {
        (Ok(Some(v)), _, _) => Some(v),
        (Ok(None), Some(a), Some(m)) => Some(vente_depuis_marge(a, m)),
        (Ok(None), _, _) => None,
        (Err(()), _, _) => {
            erreurs.push("Le prix de vente n'est pas un nombre.".to_string());
            None
        }
    };
    let remarque = s.remarque.trim().to_string();
    if remarque.chars().count() > LONGUEUR_MAX_DESCRIPTION {
        erreurs.push("La remarque est trop longue (5 000 caractères au maximum).".to_string());
    }
    match (erreurs.is_empty(), genre) {
        (true, Some(genre)) => Ok(Valeurs {
            description,
            genre,
            categorie: s.categorie.trim().to_string(),
            fournisseur: s.fournisseur.trim().to_string(),
            unite: s.unite.trim().to_string(),
            prix_achat_e5: achat,
            prix_vente_e5: vente,
            remarque,
        }),
        _ => Err(erreurs),
    }
}

#[derive(Template)]
#[template(path = "bibliotheque_fiche.html")]
struct PageFiche {
    id: Option<i64>,
    saisie: Saisie,
    genres: Vec<Choix>,
    categories: Vec<String>,
    marge_calculee: String,
    erreurs: Vec<String>,
}

async fn page_fiche(db: &mut PgConnection, id: Option<i64>, saisie: Saisie, erreurs: Vec<String>) -> Result<Html<String>, Erreur> {
    let categories: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT categorie FROM ouvrages WHERE categorie <> '' ORDER BY categorie").fetch_all(&mut *db).await?;
    let achat = calcul::lire_decimal_e5(&saisie.prix_achat);
    let marge_calculee = match calcul::lire_decimal_e5(&saisie.prix_vente) {
        Some(v) => marge_texte(achat, Some(v)),
        None => String::new(),
    };
    let page = PageFiche {
        id,
        genres: GENRES
            .iter()
            .map(|(v, l)| Choix { valeur: (*v).into(), libelle: crate::profil::type_ligne(l).into(), choisi: *v == saisie.genre })
            .collect(),
        categories,
        marge_calculee,
        erreurs,
        saisie,
    };
    Ok(Html(page.render()?))
}

async fn nouveau(mut db: Base) -> Result<Html<String>, Erreur> {
    page_fiche(&mut *db, None, Saisie { genre: "ouvrage".into(), ..Default::default() }, Vec::new()).await
}

async fn enregistrer(db: &mut PgConnection, id: Option<i64>, v: &Valeurs) -> Result<i64, sqlx::Error> {
    let sql = match id {
        None => {
            "INSERT INTO ouvrages (description, genre, categorie, fournisseur, unite, prix_achat_e5, prix_vente_e5, remarque)
             VALUES ($1, $2::text::genre_ligne, $3, $4, $5, $6, $7, $8) RETURNING id"
        }
        Some(_) => {
            // Un ouvrage venu d'une bibliothèque de Raven et changé ici est « personnalisé » : il reste si l'on décoche
            // sa bibliothèque (Réglages › Bibliothèques).
            "UPDATE ouvrages SET description = $1, genre = $2::text::genre_ligne, categorie = $3, fournisseur = $4, unite = $5,
                    prix_achat_e5 = $6, prix_vente_e5 = $7, remarque = $8, updated_at = now(),
                    personnalise = personnalise OR (description, genre::text, categorie, fournisseur, unite, prix_achat_e5, prix_vente_e5, remarque)
                                   IS DISTINCT FROM ($1, $2, $3, $4, $5, $6, $7, $8)
             WHERE id = $9 RETURNING id"
        }
    };
    sqlx::query_scalar(sql)
        .bind(&v.description)
        .bind(v.genre)
        .bind(&v.categorie)
        .bind(&v.fournisseur)
        .bind(&v.unite)
        .bind(v.prix_achat_e5)
        .bind(v.prix_vente_e5)
        .bind(&v.remarque)
        .bind(id)
        .fetch_one(&mut *db)
        .await
}

/// Ajoute un ouvrage (import d'un fichier, `bibliotheque_import`).
pub(crate) async fn inserer<'e>(db: impl sqlx::PgExecutor<'e>, v: &Valeurs) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO ouvrages (description, genre, categorie, fournisseur, unite, prix_achat_e5, prix_vente_e5, remarque)
         VALUES ($1, $2::text::genre_ligne, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(&v.description)
    .bind(v.genre)
    .bind(&v.categorie)
    .bind(&v.fournisseur)
    .bind(&v.unite)
    .bind(v.prix_achat_e5)
    .bind(v.prix_vente_e5)
    .bind(&v.remarque)
    .fetch_one(db)
    .await
}

async fn creer(mut db: Base, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    match verifier(&s) {
        Ok(v) => {
            enregistrer(&mut *db, None, &v).await?;
            Ok(Redirect::to("/bibliotheque?ok=cree").into_response())
        }
        Err(erreurs) => Ok(page_fiche(&mut *db, None, s, erreurs).await?.into_response()),
    }
}

async fn afficher(mut db: Base, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let ligne: Option<(String, String, String, String, String, Option<i64>, Option<i64>, String)> = sqlx::query_as(
        "SELECT description, genre::text, categorie, fournisseur, unite, prix_achat_e5, prix_vente_e5, remarque FROM ouvrages WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let (description, genre, categorie, fournisseur, unite, achat, vente, remarque) = ligne.ok_or_else(Erreur::introuvable)?;
    let saisie = Saisie {
        description,
        genre,
        categorie,
        fournisseur,
        unite,
        prix_achat: achat.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
        marge: String::new(),
        prix_vente: vente.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
        remarque,
    };
    page_fiche(&mut *db, Some(id), saisie, Vec::new()).await
}

async fn modifier(mut db: Base, Path(id): Path<i64>, Form(s): Form<Saisie>) -> Result<Response, Erreur> {
    let existe: Option<i64> = sqlx::query_scalar("SELECT id FROM ouvrages WHERE id = $1").bind(id).fetch_optional(&mut *db).await?;
    existe.ok_or_else(Erreur::introuvable)?;
    match verifier(&s) {
        Ok(v) => {
            enregistrer(&mut *db, Some(id), &v).await?;
            Ok(Redirect::to("/bibliotheque?ok=modifie").into_response())
        }
        Err(erreurs) => Ok(page_fiche(&mut *db, Some(id), s, erreurs).await?.into_response()),
    }
}

async fn supprimer(mut db: Base, Path(id): Path<i64>) -> Result<Redirect, Erreur> {
    let fait = sqlx::query("DELETE FROM ouvrages WHERE id = $1").bind(id).execute(&mut *db).await?;
    if fait.rows_affected() == 0 {
        return Err(Erreur::introuvable());
    }
    Ok(Redirect::to("/bibliotheque?ok=supprime"))
}

// ─── Choisir un ouvrage depuis l'Encodage d'un devis ────────────────────────

#[derive(Template)]
#[template(path = "_ouvrages_choix.html")]
struct ChoixOuvrages {
    devis: i64,
    lignes: Vec<Ligne>,
    q: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RechercheChoix {
    devis: i64,
    q: String,
}

/// Les 15 premiers ouvrages qui correspondent à la recherche, avec un bouton « Insérer » (fragment htmx).
async fn choisir(mut db: Base, Query(r): Query<RechercheChoix>) -> Result<Html<String>, Erreur> {
    let q = r.q.trim().to_string();
    let motif = format!("%{}%", echapper_like(&q));
    let lignes: Vec<OuvrageLu> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT id, description, genre::text AS genre, categorie, fournisseur, unite, prix_achat_e5, prix_vente_e5
         {DEPUIS} ORDER BY lower(description), id LIMIT 15"
    )))
    .bind(&q)
    .bind(&motif)
    .bind("")
    .fetch_all(&mut *db)
    .await?;
    Ok(Html(ChoixOuvrages { devis: r.devis, lignes: lignes.into_iter().map(Ligne::from).collect(), q }.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/bibliotheque", get(liste).post(creer))
        .route("/bibliotheque/nouveau", get(nouveau))
        .route("/bibliotheque/choisir", get(choisir))
        .route("/bibliotheque/{id}", get(afficher).post(modifier))
        .route("/bibliotheque/{id}/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saisie(achat: &str, marge: &str, vente: &str) -> Saisie {
        Saisie {
            description: "Panneaux isolants PIR 12 cm".into(),
            genre: "fourniture".into(),
            unite: "m²".into(),
            prix_achat: achat.into(),
            marge: marge.into(),
            prix_vente: vente.into(),
            ..Default::default()
        }
    }

    #[test]
    fn la_marge_et_le_prix_de_vente() {
        // 29 € d'achat, 42,50 € de vente : 46,55 % de marge.
        assert_eq!(marge_e5(2_900_000, 4_250_000), Some(4_655_172));
        assert_eq!(marge_texte(Some(2_900_000), Some(4_250_000)), "46,55 %");
        assert_eq!(marge_texte(None, Some(4_250_000)), "—");
        assert_eq!(marge_texte(Some(2_900_000), None), "—", "prix de vente à compléter");
        // 28 € + 60 % = 44,80 €.
        assert_eq!(vente_depuis_marge(2_800_000, 6_000_000), 4_480_000);
    }

    #[test]
    fn la_saisie_d_un_ouvrage() {
        let v = verifier(&saisie("28", "60", "")).unwrap();
        assert_eq!((v.prix_achat_e5, v.prix_vente_e5, v.genre), (Some(2_800_000), Some(4_480_000), "fourniture"));
        // Le prix de vente saisi l'emporte sur la marge.
        assert_eq!(verifier(&saisie("28", "60", "45")).unwrap().prix_vente_e5, Some(4_500_000));
        assert_eq!(verifier(&saisie("", "", "45")).unwrap().prix_achat_e5, None);
        // Sans prix de vente (ni marge), il reste vide : « à compléter », jamais 0 €.
        assert_eq!(verifier(&saisie("28", "", "")).unwrap().prix_vente_e5, None);
        assert_eq!(verifier(&saisie("", "", "")).unwrap().prix_vente_e5, None);
        assert!(verifier(&saisie("-1", "", "45")).is_err());
        assert!(verifier(&saisie("abc", "", "45")).is_err());
        assert!(verifier(&Saisie { genre: "titre".into(), ..saisie("", "", "45") }).is_err(), "un titre n'est pas un ouvrage");
        assert!(verifier(&Saisie { description: " ".into(), ..saisie("", "", "45") }).is_err());
    }
}
