//! Éditeur de devis, onglet Encodage (étape 5b-2) : titres, sous-titres, textes et postes libres.
//! Chaque changement est enregistré tout de suite ; le serveur recalcule les totaux.

use askama::Template;
use axum::{
    Router,
    extract::{Form as FormSimple, Path, State},
    response::Html,
    routing::{get, post},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};

use crate::calcul::{self, Totaux};
use crate::commun::{Choix, format_euros};
use crate::devis::statut;
use crate::fiches::{Genre, Options};
use crate::{AppState, Erreur};

const GENRES: [(&str, &str); 4] =
    [("titre", "Titre"), ("sous_titre", "Sous-titre"), ("texte", "Texte"), ("poste_libre", "Poste libre")];
const LONGUEUR_MAX_DESCRIPTION: usize = 5_000;
const LONGUEUR_MAX_UNITE: usize = 20;
const MESSAGE_NON_MODIFIABLE: &str = "Ce devis n'est plus un brouillon : il ne peut plus être modifié.";

// ─── Ce que l'écran affiche ─────────────────────────────────────────────────

struct Entete {
    numero: String,
    statut: &'static str,
    classe_statut: &'static str,
    brouillon: bool,
    taux_par_defaut: Option<i64>,
}

async fn charger_entete(db: &PgPool, id: i64) -> Result<Entete, Erreur> {
    let ligne: Option<(i64, i32, String, Option<i64>)> = sqlx::query_as(
        "SELECT numero, extract(year FROM date_devis)::int, statut::text, taux_tva_id FROM devis WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let (numero, annee, statut_devis, taux_par_defaut) = ligne.ok_or_else(Erreur::introuvable)?;
    let (libelle, classe) = statut(&statut_devis);
    Ok(Entete {
        numero: format!("{annee}-{numero}"),
        statut: libelle,
        classe_statut: classe,
        brouillon: statut_devis == "brouillon",
        taux_par_defaut,
    })
}

#[derive(FromRow)]
struct LigneBase {
    id: i64,
    genre: String,
    description: String,
    quantite_e5: Option<i64>,
    unite: String,
    prix_unitaire_e5: Option<i64>,
    taux_tva_id: Option<i64>,
    taux_centiemes: Option<i32>,
}

impl LigneBase {
    fn est_poste(&self) -> bool {
        self.genre == "poste_libre"
    }

    /// Total HT de la ligne (0 tant que la quantité ou le prix manque).
    fn total_cents(&self) -> i64 {
        match (self.quantite_e5, self.prix_unitaire_e5) {
            (Some(q), Some(p)) if self.est_poste() => calcul::total_ligne_cents(q, p),
            _ => 0,
        }
    }
}

async fn charger_lignes<'e>(db: impl sqlx::PgExecutor<'e>, devis_id: i64) -> Result<Vec<LigneBase>, sqlx::Error> {
    sqlx::query_as(
        "SELECT l.id, l.genre::text AS genre, l.description, l.quantite_e5, l.unite, l.prix_unitaire_e5,
                l.taux_tva_id, t.taux_centiemes
         FROM devis_lignes l LEFT JOIN taux_tva t ON t.id = l.taux_tva_id
         WHERE l.devis_id = $1 ORDER BY l.position",
    )
    .bind(devis_id)
    .fetch_all(db)
    .await
}

/// Totaux du devis à partir de ses lignes. Une ligne sans taux de TVA n'est pas comptée
/// (le serveur ne laisse pas créer de poste libre sans taux).
fn totaux_des_lignes(lignes: &[LigneBase]) -> Totaux {
    let comptees: Vec<(i64, i32)> =
        lignes.iter().filter(|l| l.est_poste()).filter_map(|l| Some((l.total_cents(), l.taux_centiemes?))).collect();
    calcul::totaux(&comptees)
}

pub struct LigneVue {
    pub id: i64,
    pub genre: &'static str,
    pub libelle_genre: &'static str,
    pub est_titre: bool,
    pub est_poste: bool,
    pub numero: String,
    pub description: String,
    pub quantite: String,
    pub unite: String,
    pub prix_unitaire: String,
    pub taux: Vec<Choix>,
    pub total: String,
    pub sous_total: String,
    pub premiere: bool,
    pub derniere: bool,
}

pub struct TvaVue {
    pub taux: String,
    pub base: String,
    pub montant: String,
}

pub struct Encodage {
    pub id: i64,
    pub modifiable: bool,
    pub lignes: Vec<LigneVue>,
    pub ht: String,
    pub tva: Vec<TvaVue>,
    pub ttc: String,
    /// Message affiché au-dessus du tableau (erreur de saisie, par exemple).
    pub message: String,
}

fn genre_connu(genre: &str) -> Option<(&'static str, &'static str)> {
    GENRES.iter().find(|(v, _)| *v == genre).copied()
}

fn construire_encodage(id: i64, modifiable: bool, lignes: &[LigneBase], taux: &[(i64, String)], message: String) -> Encodage {
    // Numérotation : un titre ouvre un lot (1.0, 2.0…), les lignes suivantes en font partie (1.1, 1.2…).
    // Le sous-total d'un titre est la somme des postes jusqu'au titre suivant.
    let mut vues = Vec::with_capacity(lignes.len());
    let (mut lot, mut rang) = (0, 0);
    let mut sous_totaux: Vec<(usize, i64)> = Vec::new();
    for (i, l) in lignes.iter().enumerate() {
        let (libelle_genre, genre) = match genre_connu(&l.genre) {
            Some((v, lib)) => (lib, v),
            None => ("?", "texte"),
        };
        let est_titre = l.genre == "titre";
        let numero = if est_titre {
            lot += 1;
            rang = 0;
            sous_totaux.push((i, 0));
            format!("{lot}.0")
        } else {
            rang += 1;
            if let Some((_, somme)) = sous_totaux.last_mut() {
                *somme += l.total_cents();
            }
            format!("{lot}.{rang}")
        };
        vues.push(LigneVue {
            id: l.id,
            genre,
            libelle_genre,
            est_titre,
            est_poste: l.est_poste(),
            numero,
            description: l.description.clone(),
            quantite: l.quantite_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
            unite: l.unite.clone(),
            prix_unitaire: l.prix_unitaire_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
            taux: taux
                .iter()
                .map(|(tid, libelle)| Choix {
                    valeur: tid.to_string(),
                    libelle: libelle.clone(),
                    choisi: Some(*tid) == l.taux_tva_id,
                })
                .collect(),
            total: if l.est_poste() { format_euros(l.total_cents()) } else { String::new() },
            sous_total: String::new(),
            premiere: i == 0,
            derniere: i + 1 == lignes.len(),
        });
    }
    for (i, somme) in sous_totaux {
        vues[i].sous_total = format_euros(somme);
    }

    let t = totaux_des_lignes(lignes);
    Encodage {
        id,
        modifiable,
        lignes: vues,
        ht: format_euros(t.ht_cents),
        tva: t
            .tva
            .iter()
            .map(|v| TvaVue {
                taux: calcul::taux_en_texte(v.taux_centiemes),
                base: format_euros(v.base_cents),
                montant: format_euros(v.tva_cents),
            })
            .collect(),
        ttc: format_euros(t.ttc_cents),
        message,
    }
}

async fn charger_encodage(db: &PgPool, id: i64, entete: &Entete, message: String) -> Result<Encodage, Erreur> {
    let lignes = charger_lignes(db, id).await?;
    let options = Options::charger(db, Genre::Entreprise).await?;
    Ok(construire_encodage(id, entete.brouillon, &lignes, &options.taux_tva, message))
}

// ─── Templates ──────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "devis_encodage.html")]
struct Page {
    id: i64,
    numero: String,
    statut: &'static str,
    classe_statut: &'static str,
    onglet: &'static str,
    enc: Encodage,
}

/// Tout le bloc d'encodage : renvoyé à htmx après un ajout, une suppression ou un déplacement.
#[derive(Template)]
#[template(path = "_encodage.html")]
struct Bloc {
    enc: Encodage,
}

/// Seulement les totaux et les messages : renvoyé après la modification d'une ligne, pour ne pas
/// toucher aux champs en cours de saisie.
#[derive(Template)]
#[template(path = "_encodage_maj.html")]
struct Maj {
    enc: Encodage,
}

// ─── Modifications ──────────────────────────────────────────────────────────

fn exiger_brouillon(entete: &Entete) -> Result<(), Erreur> {
    if entete.brouillon { Ok(()) } else { Err(Erreur::conflit(MESSAGE_NON_MODIFIABLE)) }
}

/// Recalcule et enregistre les totaux du devis dans la même transaction que le changement.
async fn recalculer(tx: &mut Transaction<'_, Postgres>, devis_id: i64) -> Result<(), sqlx::Error> {
    let lignes = charger_lignes(&mut **tx, devis_id).await?;
    let t = totaux_des_lignes(&lignes);
    sqlx::query("UPDATE devis SET montant_ht_cents = $1, montant_ttc_cents = $2, updated_at = now() WHERE id = $3")
        .bind(t.ht_cents)
        .bind(t.ttc_cents)
        .bind(devis_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn page(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&state.db, id).await?;
    let enc = charger_encodage(&state.db, id, &entete, String::new()).await?;
    let page = Page { id, numero: entete.numero, statut: entete.statut, classe_statut: entete.classe_statut, onglet: "encodage", enc };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NouvelleLigne {
    genre: String,
}

async fn ajouter(State(state): State<AppState>, Path(id): Path<i64>, Form(f): Form<NouvelleLigne>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&state.db, id).await?;
    exiger_brouillon(&entete)?;
    let Some((genre, _)) = genre_connu(&f.genre) else {
        let enc = charger_encodage(&state.db, id, &entete, "Ce type de ligne n'existe pas.".to_string()).await?;
        return Ok(Html(Bloc { enc }.render()?));
    };
    if genre == "poste_libre" && entete.taux_par_defaut.is_none() {
        let message = "Choisis d'abord le taux de TVA par défaut dans l'onglet Informations.".to_string();
        let enc = charger_encodage(&state.db, id, &entete, message).await?;
        return Ok(Html(Bloc { enc }.render()?));
    }

    let mut tx = state.db.begin().await?;
    // Verrouille le devis : deux ajouts en même temps ne prennent pas la même position.
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let est_poste = genre == "poste_libre";
    sqlx::query(
        "INSERT INTO devis_lignes (devis_id, position, genre, quantite_e5, taux_tva_id)
         VALUES ($1, COALESCE((SELECT max(position) FROM devis_lignes WHERE devis_id = $1), 0) + 1,
                 $2::text::genre_ligne, $3, $4)",
    )
    .bind(id)
    .bind(genre)
    .bind(est_poste.then_some(100_000_i64))
    .bind(if est_poste { entete.taux_par_defaut } else { None })
    .execute(&mut *tx)
    .await?;
    recalculer(&mut tx, id).await?;
    tx.commit().await?;

    let enc = charger_encodage(&state.db, id, &entete, String::new()).await?;
    Ok(Html(Bloc { enc }.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MajLigne {
    description: Option<String>,
    quantite: Option<String>,
    unite: Option<String>,
    prix_unitaire: Option<String>,
    taux_tva_id: Option<String>,
}

/// Applique les champs reçus à la ligne ; le premier problème rencontré est renvoyé tel quel.
fn appliquer(l: &mut LigneBase, f: &MajLigne, taux: &[(i64, String)]) -> Result<(), String> {
    if let Some(d) = &f.description {
        if d.chars().count() > LONGUEUR_MAX_DESCRIPTION {
            return Err(format!("La description est trop longue ({LONGUEUR_MAX_DESCRIPTION} caractères au maximum)."));
        }
        l.description = d.trim_end().to_string();
    }
    if !l.est_poste() {
        return Ok(());
    }
    if let Some(q) = &f.quantite {
        l.quantite_e5 = match q.trim() {
            "" => None,
            t => Some(calcul::lire_decimal_e5(t).ok_or_else(|| format!("La quantité « {t} » n'est pas un nombre (exemple : 2,5)."))?),
        };
    }
    if let Some(p) = &f.prix_unitaire {
        l.prix_unitaire_e5 = match p.trim() {
            "" => None,
            t => Some(calcul::lire_decimal_e5(t).ok_or_else(|| format!("Le prix « {t} » n'est pas un montant (exemple : 19,99)."))?),
        };
    }
    if let Some(u) = &f.unite {
        let u = u.trim();
        if u.chars().count() > LONGUEUR_MAX_UNITE {
            return Err(format!("L'unité est trop longue ({LONGUEUR_MAX_UNITE} caractères au maximum)."));
        }
        l.unite = u.to_string();
    }
    if let Some(t) = &f.taux_tva_id {
        let id = t.trim().parse::<i64>().ok().filter(|id| taux.iter().any(|(i, _)| i == id)).ok_or("Ce taux de TVA n'existe pas.")?;
        l.taux_tva_id = Some(id);
    }
    Ok(())
}

async fn modifier(
    State(state): State<AppState>,
    Path((id, ligne_id)): Path<(i64, i64)>,
    // Formulaire « simple » : un champ vidé arrive comme texte vide (et non comme champ absent).
    FormSimple(f): FormSimple<MajLigne>,
) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&state.db, id).await?;
    exiger_brouillon(&entete)?;
    let options = Options::charger(&state.db, Genre::Entreprise).await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let courante: Option<LigneBase> = sqlx::query_as(
        "SELECT l.id, l.genre::text AS genre, l.description, l.quantite_e5, l.unite, l.prix_unitaire_e5,
                l.taux_tva_id, NULL::int AS taux_centiemes
         FROM devis_lignes l WHERE l.id = $1 AND l.devis_id = $2",
    )
    .bind(ligne_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let mut ligne = courante.ok_or_else(Erreur::introuvable)?;

    let message = match appliquer(&mut ligne, &f, &options.taux_tva) {
        Ok(()) => {
            sqlx::query(
                "UPDATE devis_lignes SET description = $1, quantite_e5 = $2, unite = $3, prix_unitaire_e5 = $4,
                        taux_tva_id = $5, updated_at = now() WHERE id = $6",
            )
            .bind(&ligne.description)
            .bind(ligne.quantite_e5)
            .bind(&ligne.unite)
            .bind(ligne.prix_unitaire_e5)
            .bind(ligne.taux_tva_id)
            .bind(ligne_id)
            .execute(&mut *tx)
            .await?;
            recalculer(&mut tx, id).await?;
            String::new()
        }
        // Rien n'est enregistré : le champ garde ce que la personne a tapé, avec l'explication en haut.
        Err(m) => m,
    };
    tx.commit().await?;

    let enc = charger_encodage(&state.db, id, &entete, message).await?;
    Ok(Html(Maj { enc }.render()?))
}

async fn supprimer(State(state): State<AppState>, Path((id, ligne_id)): Path<(i64, i64)>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&state.db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    sqlx::query("DELETE FROM devis_lignes WHERE id = $1 AND devis_id = $2").bind(ligne_id).bind(id).execute(&mut *tx).await?;
    recalculer(&mut tx, id).await?;
    tx.commit().await?;
    let enc = charger_encodage(&state.db, id, &entete, String::new()).await?;
    Ok(Html(Bloc { enc }.render()?))
}

/// Échange la ligne avec sa voisine (`vers_le_haut` : celle du dessus). Sans voisine, ne fait rien.
async fn deplacer(state: AppState, id: i64, ligne_id: i64, vers_le_haut: bool) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&state.db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let position: Option<i32> = sqlx::query_scalar("SELECT position FROM devis_lignes WHERE id = $1 AND devis_id = $2")
        .bind(ligne_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let position = position.ok_or_else(Erreur::introuvable)?;
    let voisine_sql = if vers_le_haut {
        "SELECT id, position FROM devis_lignes WHERE devis_id = $1 AND position < $2 ORDER BY position DESC LIMIT 1"
    } else {
        "SELECT id, position FROM devis_lignes WHERE devis_id = $1 AND position > $2 ORDER BY position LIMIT 1"
    };
    let voisine: Option<(i64, i32)> = sqlx::query_as(voisine_sql).bind(id).bind(position).fetch_optional(&mut *tx).await?;
    if let Some((voisine_id, voisine_position)) = voisine {
        // La contrainte d'unicité n'est vérifiée qu'à la fin de la transaction : l'échange est permis.
        sqlx::query("UPDATE devis_lignes SET position = CASE id WHEN $1 THEN $4 ELSE $3 END WHERE id IN ($1, $2)")
            .bind(ligne_id)
            .bind(voisine_id)
            .bind(position)
            .bind(voisine_position)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    let enc = charger_encodage(&state.db, id, &entete, String::new()).await?;
    Ok(Html(Bloc { enc }.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devis/{id}/encodage", get(page))
        .route("/devis/{id}/lignes", post(ajouter))
        .route("/devis/{id}/lignes/{ligne_id}", post(modifier))
        .route("/devis/{id}/lignes/{ligne_id}/supprimer", post(supprimer))
        .route(
            "/devis/{id}/lignes/{ligne_id}/monter",
            post(|State(s): State<AppState>, Path((id, l)): Path<(i64, i64)>| deplacer(s, id, l, true)),
        )
        .route(
            "/devis/{id}/lignes/{ligne_id}/descendre",
            post(|State(s): State<AppState>, Path((id, l)): Path<(i64, i64)>| deplacer(s, id, l, false)),
        )
}
