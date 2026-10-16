//! Avancements (étape 6-3) : facturation au fur et à mesure du chantier, comme dans Vertuoza.
//! Un avancement reprend toutes les lignes du devis de base puis celles de chaque avenant accepté ;
//! pour chaque ligne chiffrée, on saisit l'avancement de la période en quantité, en pourcentage ou en euros.
//! Le serveur enregistre le montant de la période (en centimes), vérifie que le cumul reste entre 0 et 100 %
//! et calcule les totaux : total du chantier, déjà avancé, période, cumul, reste à avancer.
//! Un seul brouillon à la fois par chantier ; un avancement validé ne se modifie plus.
//! Il deviendra une facture d'avancement à l'étape 7.

use std::collections::HashMap;

use askama::Template;
use axum::{
    Router,
    extract::{Form, Path, Query, State},
    response::{Html, Redirect},
    routing::{get, post},
};
use chrono::NaiveDate;
use serde::Deserialize;
use sqlx::{FromRow, PgPool};

use crate::avancement_calcul as ac;
use crate::calcul;
use crate::commun::format_euros;
use crate::{AppState, Erreur};

const LONGUEUR_MAX_REFERENCE: usize = 200;
const GENRES_CHIFFRES: [&str; 6] = ["poste_libre", "ouvrage", "fourniture", "equipement", "main_oeuvre", "sous_traitant"];

// ─── Les postes à avancer : le devis de base, puis les avenants acceptés ────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nature {
    Titre,
    Texte,
    Chiffre,
    /// La remise ou la majoration d'un devis : elle s'avance comme une ligne.
    Ajustement,
}

#[derive(Debug, Clone)]
struct Poste {
    /// « l12 » pour la ligne 12 d'un devis, « a5 » pour l'ajustement du devis 5.
    cle: String,
    numero: String,
    description: String,
    nature: Nature,
    quantite_e5: Option<i64>,
    unite: String,
    prix_e5: Option<i64>,
    total_cents: i64,
}

impl Poste {
    fn avancable(&self) -> bool {
        matches!(self.nature, Nature::Chiffre | Nature::Ajustement) && self.total_cents != 0
    }
}

struct Bloc {
    titre: String,
    postes: Vec<Poste>,
    /// Le montant HT du devis ou de l'avenant (ajustement compris).
    total_cents: i64,
}

#[derive(FromRow)]
struct DevisDuChantier {
    id: i64,
    titre: String,
    montant_ht_cents: i64,
    ajustement_libelle: String,
}

#[derive(FromRow)]
struct LigneDevis {
    id: i64,
    devis_id: i64,
    genre: String,
    description: String,
    quantite_e5: Option<i64>,
    unite: String,
    prix_unitaire_e5: Option<i64>,
    taux_tva_id: Option<i64>,
}

/// Numérote et chiffre les lignes d'un devis comme dans son éditeur (1.0, 1.1…), sans les sauts de page,
/// et ajoute l'ajustement du devis comme poste à part quand il y en a un.
fn construire_bloc(devis: &DevisDuChantier, lignes: &[&LigneDevis]) -> Bloc {
    let (mut lot, mut rang) = (0, 0);
    let mut postes = Vec::new();
    let mut somme = 0;
    for l in lignes {
        let numero = match l.genre.as_str() {
            "saut_de_page" => continue,
            "titre" => {
                lot += 1;
                rang = 0;
                format!("{lot}.0")
            }
            _ => {
                rang += 1;
                format!("{lot}.{rang}")
            }
        };
        let chiffre = GENRES_CHIFFRES.contains(&l.genre.as_str());
        // Comme dans les totaux du devis : une ligne sans taux de TVA ne compte pas.
        let total = match (chiffre, l.quantite_e5, l.prix_unitaire_e5, l.taux_tva_id) {
            (true, Some(q), Some(p), Some(_)) => calcul::total_ligne_cents(q, p),
            _ => 0,
        };
        somme += total;
        postes.push(Poste {
            cle: format!("l{}", l.id),
            numero,
            description: l.description.clone(),
            nature: if l.genre == "titre" {
                Nature::Titre
            } else if chiffre {
                Nature::Chiffre
            } else {
                Nature::Texte
            },
            quantite_e5: if chiffre { l.quantite_e5 } else { None },
            unite: l.unite.clone(),
            prix_e5: if chiffre { l.prix_unitaire_e5 } else { None },
            total_cents: total,
        });
    }
    // Le montant HT du devis comprend son ajustement : ce qui manque aux lignes, c'est l'ajustement.
    let ajustement = devis.montant_ht_cents - somme;
    if ajustement != 0 {
        postes.push(Poste {
            cle: format!("a{}", devis.id),
            numero: String::new(),
            description: if devis.ajustement_libelle.is_empty() { "Ajustement".to_string() } else { devis.ajustement_libelle.clone() },
            nature: Nature::Ajustement,
            quantite_e5: None,
            unite: String::new(),
            prix_e5: None,
            total_cents: ajustement,
        });
    }
    Bloc { titre: devis.titre.clone(), postes, total_cents: devis.montant_ht_cents }
}

async fn charger_blocs(db: &PgPool, chantier_id: i64) -> Result<Vec<Bloc>, sqlx::Error> {
    let devis: Vec<DevisDuChantier> = sqlx::query_as(
        "SELECT d.id,
                CASE WHEN d.chantier_id IS NULL THEN 'Devis ' || extract(year FROM d.date_devis)::int || '-' || d.numero
                     ELSE 'Avenant n° ' || d.numero END AS titre,
                d.montant_ht_cents, d.ajustement_libelle
         FROM devis d JOIN chantiers ch ON ch.id = $1
         WHERE d.id = ch.devis_id OR (d.chantier_id = ch.id AND d.statut = 'accepte')
         ORDER BY d.chantier_id IS NOT NULL, d.numero",
    )
    .bind(chantier_id)
    .fetch_all(db)
    .await?;
    let ids: Vec<i64> = devis.iter().map(|d| d.id).collect();
    let lignes: Vec<LigneDevis> = sqlx::query_as(
        "SELECT id, devis_id, genre::text AS genre, description, quantite_e5, unite, prix_unitaire_e5, taux_tva_id
         FROM devis_lignes WHERE devis_id = ANY($1) ORDER BY position",
    )
    .bind(&ids)
    .fetch_all(db)
    .await?;
    Ok(devis
        .iter()
        .map(|d| {
            let siennes: Vec<&LigneDevis> = lignes.iter().filter(|l| l.devis_id == d.id).collect();
            construire_bloc(d, &siennes)
        })
        .collect())
}

/// Ce que chaque avancement du chantier a avancé, poste par poste : (numéro de l'avancement, clé du poste, montant).
async fn charger_montants(db: &PgPool, chantier_id: i64) -> Result<Vec<(i32, String, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.numero, COALESCE('l' || al.devis_ligne_id, 'a' || al.ajustement_devis_id), al.montant_cents
         FROM avancement_lignes al JOIN avancements a ON a.id = al.avancement_id
         WHERE a.chantier_id = $1",
    )
    .bind(chantier_id)
    .fetch_all(db)
    .await
}

/// Le précédent (avancements de numéro plus petit) et la période (cet avancement), par poste.
fn repartir(montants: &[(i32, String, i64)], numero: i32) -> (HashMap<String, i64>, HashMap<String, i64>) {
    let (mut precedent, mut periode) = (HashMap::new(), HashMap::new());
    for (n, cle, m) in montants {
        if *n < numero {
            *precedent.entry(cle.clone()).or_insert(0) += m;
        } else if *n == numero {
            *periode.entry(cle.clone()).or_insert(0) += m;
        }
    }
    (precedent, periode)
}

// ─── Ce que l'écran affiche ─────────────────────────────────────────────────

/// Une colonne « quantité, pourcentage, montant » (précédent, période ou cumul).
pub struct Trio {
    pub quantite: String,
    pub pourcentage: String,
    pub montant: String,
}

fn trio(p: &Poste, montant: i64, pour_saisie: bool) -> Trio {
    let quantite = match p.quantite_e5 {
        Some(q) => ac::quantite_depuis_montant(montant, p.total_cents, q).map(calcul::ecrire_decimal_e5).unwrap_or_default(),
        None => String::new(),
    };
    let centiemes = ac::pourcentage_centiemes(montant, p.total_cents).unwrap_or(0);
    if pour_saisie {
        Trio { quantite, pourcentage: ac::pourcentage_a_saisir(centiemes), montant: ac::euros_a_saisir(montant) }
    } else {
        Trio { quantite, pourcentage: ac::pourcentage_en_texte(centiemes), montant: format_euros(montant) }
    }
}

pub struct LigneVue {
    pub cle: String,
    /// Où envoyer la saisie de cette ligne.
    pub adresse: String,
    pub numero: String,
    pub description: String,
    pub est_titre: bool,
    /// Ligne chiffrée (ou ajustement) au total non nul : elle s'avance.
    pub avancable: bool,
    /// L'ajustement n'a pas de quantité.
    pub a_quantite: bool,
    pub quantite: String,
    pub unite: String,
    pub prix: String,
    pub total: String,
    pub precedent: Trio,
    pub periode: Trio,
    /// La période telle qu'elle apparaît dans les champs de saisie (« 55,04 », « 3634,47 »).
    pub saisie: Trio,
    pub cumul: Trio,
}

pub struct BlocVue {
    pub titre: String,
    pub lignes: Vec<LigneVue>,
    pub total: String,
    pub precedent: String,
    pub periode: String,
    pub cumul: String,
}

pub struct Grille {
    pub chantier_id: i64,
    pub avancement_id: i64,
    pub modifiable: bool,
    pub message: String,
    pub blocs: Vec<BlocVue>,
    pub total: String,
    pub precedent: String,
    pub periode: String,
    pub cumul: String,
    pub pourcentage_cumul: String,
    pub reste: String,
}

/// Les sommes d'un bloc ou de tout le chantier, en centimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Sommes {
    total: i64,
    precedent: i64,
    periode: i64,
}

fn construire_grille(chantier_id: i64, avancement_id: i64, modifiable: bool, message: String, blocs: &[Bloc], precedent: &HashMap<String, i64>, periode: &HashMap<String, i64>) -> Grille {
    let mut s = Sommes::default();
    let mut vues = Vec::new();
    for b in blocs {
        let mut sb = Sommes { total: b.total_cents, ..Sommes::default() };
        let lignes = b
            .postes
            .iter()
            .map(|p| {
                let prec = precedent.get(&p.cle).copied().unwrap_or(0);
                let per = periode.get(&p.cle).copied().unwrap_or(0);
                sb.precedent += prec;
                sb.periode += per;
                let chiffre = matches!(p.nature, Nature::Chiffre | Nature::Ajustement);
                LigneVue {
                    cle: p.cle.clone(),
                    adresse: format!("/chantiers/{chantier_id}/avancements/{avancement_id}/lignes/{}", p.cle),
                    numero: p.numero.clone(),
                    description: p.description.clone(),
                    est_titre: p.nature == Nature::Titre,
                    avancable: p.avancable(),
                    a_quantite: p.quantite_e5.is_some() && p.prix_e5.is_some(),
                    quantite: p.quantite_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
                    unite: p.unite.clone(),
                    prix: p.prix_e5.map(calcul::prix_pour_affichage).unwrap_or_default(),
                    total: if chiffre { format_euros(p.total_cents) } else { String::new() },
                    precedent: trio(p, prec, false),
                    periode: trio(p, per, false),
                    saisie: trio(p, per, true),
                    cumul: trio(p, prec + per, false),
                }
            })
            .collect();
        s.total += sb.total;
        s.precedent += sb.precedent;
        s.periode += sb.periode;
        vues.push(BlocVue {
            titre: b.titre.clone(),
            lignes,
            total: format_euros(sb.total),
            precedent: format_euros(sb.precedent),
            periode: format_euros(sb.periode),
            cumul: format_euros(sb.precedent + sb.periode),
        });
    }
    let cumul = s.precedent + s.periode;
    Grille {
        chantier_id,
        avancement_id,
        modifiable,
        message,
        blocs: vues,
        total: format_euros(s.total),
        precedent: format_euros(s.precedent),
        periode: format_euros(s.periode),
        cumul: format_euros(cumul),
        pourcentage_cumul: ac::pourcentage_centiemes(cumul, s.total).map(ac::pourcentage_en_texte).unwrap_or_default(),
        reste: format_euros(s.total - cumul),
    }
}

#[derive(FromRow)]
struct Avancement {
    numero: i32,
    date_avancement: NaiveDate,
    reference: String,
    statut: String,
    chantier_numero: i64,
    /// Le numéro du dernier avancement du chantier.
    dernier: i32,
}

async fn charger_avancement(db: &PgPool, chantier_id: i64, id: i64) -> Result<Avancement, Erreur> {
    let a: Option<Avancement> = sqlx::query_as(
        "SELECT a.numero, a.date_avancement, a.reference, a.statut::text AS statut, ch.numero AS chantier_numero,
                (SELECT max(numero) FROM avancements WHERE chantier_id = a.chantier_id) AS dernier
         FROM avancements a JOIN chantiers ch ON ch.id = a.chantier_id
         WHERE a.id = $1 AND a.chantier_id = $2",
    )
    .bind(id)
    .bind(chantier_id)
    .fetch_optional(db)
    .await?;
    a.ok_or_else(Erreur::introuvable)
}

async fn charger_grille(db: &PgPool, chantier_id: i64, id: i64, a: &Avancement, message: String) -> Result<Grille, Erreur> {
    let blocs = charger_blocs(db, chantier_id).await?;
    let montants = charger_montants(db, chantier_id).await?;
    let (precedent, periode) = repartir(&montants, a.numero);
    Ok(construire_grille(chantier_id, id, a.statut == "brouillon", message, &blocs, &precedent, &periode))
}

#[derive(Template)]
#[template(path = "avancement.html")]
struct Page {
    chantier_id: i64,
    chantier_numero: i64,
    id: i64,
    numero: i32,
    reference: String,
    date: String,
    /// La même date, au format du champ de formulaire.
    date_champ: String,
    brouillon: bool,
    peut_remettre_en_brouillon: bool,
    message_ok: &'static str,
    message_erreur: &'static str,
    grille: Grille,
}

#[derive(Template)]
#[template(path = "_avancement_grille.html")]
struct GrilleSeule {
    grille: Grille,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Retour {
    ok: String,
    erreur: String,
}

impl Retour {
    fn message_ok(&self) -> &'static str {
        match self.ok.as_str() {
            "cree" => "L'avancement est créé : saisis ce qui a été réalisé, en quantité, en % ou en €.",
            "infos" => "Les informations sont enregistrées.",
            "valide" => "L'avancement est validé : il ne se modifie plus.",
            "brouillon" => "L'avancement est remis en brouillon.",
            _ => "",
        }
    }

    fn message_erreur(&self) -> &'static str {
        match self.erreur.as_str() {
            "date" => "La date de l'avancement n'est pas valide.",
            "reference" => "La référence est obligatoire (200 caractères au maximum).",
            "vide" => "L'avancement est vide : saisis au moins une ligne avant de le valider.",
            "brouillon" => "Termine ou supprime d'abord l'avancement en brouillon.",
            _ => "",
        }
    }
}

async fn afficher(State(state): State<AppState>, Path((chantier_id, id)): Path<(i64, i64)>, Query(retour): Query<Retour>) -> Result<Html<String>, Erreur> {
    let a = charger_avancement(&state.db, chantier_id, id).await?;
    let grille = charger_grille(&state.db, chantier_id, id, &a, String::new()).await?;
    let page = Page {
        chantier_id,
        chantier_numero: a.chantier_numero,
        id,
        numero: a.numero,
        reference: a.reference.clone(),
        date: a.date_avancement.format("%d/%m/%Y").to_string(),
        date_champ: a.date_avancement.to_string(),
        brouillon: a.statut == "brouillon",
        peut_remettre_en_brouillon: a.statut == "valide" && a.numero == a.dernier,
        message_ok: retour.message_ok(),
        message_erreur: retour.message_erreur(),
        grille,
    };
    Ok(Html(page.render()?))
}

// ─── Créer, saisir, valider ─────────────────────────────────────────────────

/// Crée l'avancement suivant en brouillon, ou rouvre le brouillon qui existe déjà.
async fn creer(State(state): State<AppState>, Path(chantier_id): Path<i64>) -> Result<Redirect, Erreur> {
    let mut tx = state.db.begin().await?;
    // Verrouille le chantier : deux créations en même temps n'ont pas le même numéro.
    let existe: Option<i64> =
        sqlx::query_scalar("SELECT id FROM chantiers WHERE id = $1 FOR UPDATE").bind(chantier_id).fetch_optional(&mut *tx).await?;
    existe.ok_or_else(Erreur::introuvable)?;
    let brouillon: Option<i64> = sqlx::query_scalar("SELECT id FROM avancements WHERE chantier_id = $1 AND statut = 'brouillon'")
        .bind(chantier_id)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(id) = brouillon {
        return Ok(Redirect::to(&format!("/chantiers/{chantier_id}/avancements/{id}?erreur=brouillon")));
    }
    let numero: i32 = sqlx::query_scalar("SELECT COALESCE(max(numero), 0) + 1 FROM avancements WHERE chantier_id = $1")
        .bind(chantier_id)
        .fetch_one(&mut *tx)
        .await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO avancements (chantier_id, numero, date_avancement, reference)
         VALUES ($1, $2, (now() AT TIME ZONE 'Europe/Brussels')::date, $3) RETURNING id",
    )
    .bind(chantier_id)
    .bind(numero)
    .bind(format!("Avancement n° {numero}"))
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Redirect::to(&format!("/chantiers/{chantier_id}/avancements/{id}?ok=cree")))
}

/// Un seul des trois champs arrive : celui que l'utilisateur vient de changer.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Saisie {
    quantite: Option<String>,
    pourcentage: Option<String>,
    montant: Option<String>,
}

/// Le montant de la période pour ce poste, d'après le champ saisi. Un champ vide vaut 0.
fn lire_saisie(s: &Saisie, p: &Poste) -> Result<i64, String> {
    let nombre = |texte: &str, exemple: &str| -> Result<Option<i64>, String> {
        match texte.trim() {
            "" => Ok(None),
            t => calcul::lire_decimal_e5(t).map(Some).ok_or_else(|| format!("« {t} » n'est pas un nombre (exemple : {exemple}).")),
        }
    };
    if let Some(t) = &s.montant {
        return Ok(nombre(t, "1250,50")?.map(ac::montant_depuis_euros).unwrap_or(0));
    }
    if let Some(t) = &s.pourcentage {
        return Ok(nombre(t, "55,04")?.map(|v| ac::montant_depuis_pourcentage(p.total_cents, v)).unwrap_or(0));
    }
    if let Some(t) = &s.quantite {
        let Some(prix) = p.prix_e5.filter(|_| p.quantite_e5.is_some()) else {
            return Err("Cette ligne s'avance en % ou en €.".to_string());
        };
        return Ok(nombre(t, "2,5")?.map(|q| ac::montant_depuis_quantite(q, prix)).unwrap_or(0));
    }
    Err("Rien à enregistrer.".to_string())
}

async fn saisir(
    State(state): State<AppState>,
    Path((chantier_id, id, cle)): Path<(i64, i64, String)>,
    Form(s): Form<Saisie>,
) -> Result<Html<String>, Erreur> {
    let a = charger_avancement(&state.db, chantier_id, id).await?;
    if a.statut != "brouillon" {
        return Err(Erreur::conflit("Cet avancement est validé : il ne se modifie plus."));
    }
    let blocs = charger_blocs(&state.db, chantier_id).await?;
    let Some(poste) = blocs.iter().flat_map(|b| b.postes.iter()).find(|p| p.cle == cle && p.avancable()).cloned() else {
        return Err(Erreur::introuvable());
    };

    let message = match lire_saisie(&s, &poste) {
        Err(m) => m,
        Ok(montant) => {
            let mut tx = state.db.begin().await?;
            sqlx::query("SELECT id FROM avancements WHERE id = $1 FOR UPDATE").bind(id).execute(&mut *tx).await?;
            let precedent: i64 = sqlx::query_scalar(
                "SELECT COALESCE(sum(al.montant_cents), 0)::bigint
                 FROM avancement_lignes al JOIN avancements x ON x.id = al.avancement_id
                 WHERE x.chantier_id = $1 AND x.numero < $2
                   AND COALESCE('l' || al.devis_ligne_id, 'a' || al.ajustement_devis_id) = $3",
            )
            .bind(chantier_id)
            .bind(a.numero)
            .bind(&cle)
            .fetch_one(&mut *tx)
            .await?;
            if !ac::cumul_permis(precedent + montant, poste.total_cents) {
                format!(
                    "Ligne {} : le cumul doit rester entre 0 et 100 % (déjà avancé : {}, total : {}).",
                    if poste.numero.is_empty() { poste.description.clone() } else { poste.numero.clone() },
                    format_euros(precedent),
                    format_euros(poste.total_cents)
                )
            } else {
                let (ligne, ajustement) = colonnes_cle(&cle);
                sqlx::query(
                    "DELETE FROM avancement_lignes WHERE avancement_id = $1
                       AND devis_ligne_id IS NOT DISTINCT FROM $2 AND ajustement_devis_id IS NOT DISTINCT FROM $3",
                )
                .bind(id)
                .bind(ligne)
                .bind(ajustement)
                .execute(&mut *tx)
                .await?;
                if montant != 0 {
                    sqlx::query("INSERT INTO avancement_lignes (avancement_id, devis_ligne_id, ajustement_devis_id, montant_cents) VALUES ($1, $2, $3, $4)")
                        .bind(id)
                        .bind(ligne)
                        .bind(ajustement)
                        .bind(montant)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query(
                    "UPDATE avancements SET updated_at = now(),
                            montant_ht_cents = (SELECT COALESCE(sum(montant_cents), 0) FROM avancement_lignes WHERE avancement_id = $1)
                     WHERE id = $1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                String::new()
            }
        }
    };
    let grille = charger_grille(&state.db, chantier_id, id, &a, message).await?;
    Ok(Html(GrilleSeule { grille }.render()?))
}

/// « l12 » → (ligne 12, pas d'ajustement) ; « a5 » → (pas de ligne, ajustement du devis 5).
fn colonnes_cle(cle: &str) -> (Option<i64>, Option<i64>) {
    let (genre, reste) = cle.split_at_checked(1).unwrap_or(("", ""));
    let n = reste.parse::<i64>().ok();
    match genre {
        "l" => (n, None),
        "a" => (None, n),
        _ => (None, None),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Infos {
    date: String,
    reference: String,
}

async fn enregistrer_infos(State(state): State<AppState>, Path((chantier_id, id)): Path<(i64, i64)>, Form(f): Form<Infos>) -> Result<Redirect, Erreur> {
    let a = charger_avancement(&state.db, chantier_id, id).await?;
    if a.statut != "brouillon" {
        return Err(Erreur::conflit("Cet avancement est validé : il ne se modifie plus."));
    }
    let retour = |code: &str| Ok(Redirect::to(&format!("/chantiers/{chantier_id}/avancements/{id}?{code}")));
    let Ok(date) = NaiveDate::parse_from_str(f.date.trim(), "%Y-%m-%d") else {
        return retour("erreur=date");
    };
    let reference = f.reference.trim();
    if reference.is_empty() || reference.chars().count() > LONGUEUR_MAX_REFERENCE {
        return retour("erreur=reference");
    }
    sqlx::query("UPDATE avancements SET date_avancement = $1, reference = $2, updated_at = now() WHERE id = $3")
        .bind(date)
        .bind(reference)
        .bind(id)
        .execute(&state.db)
        .await?;
    retour("ok=infos")
}

async fn valider(State(state): State<AppState>, Path((chantier_id, id)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    let a = charger_avancement(&state.db, chantier_id, id).await?;
    if a.statut != "brouillon" {
        return Err(Erreur::conflit("Cet avancement est déjà validé."));
    }
    let lignes: i64 = sqlx::query_scalar("SELECT count(*) FROM avancement_lignes WHERE avancement_id = $1").bind(id).fetch_one(&state.db).await?;
    if lignes == 0 {
        return Ok(Redirect::to(&format!("/chantiers/{chantier_id}/avancements/{id}?erreur=vide")));
    }
    sqlx::query("UPDATE avancements SET statut = 'valide', updated_at = now() WHERE id = $1 AND statut = 'brouillon'")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(Redirect::to(&format!("/chantiers/{chantier_id}/avancements/{id}?ok=valide")))
}

/// Seul le dernier avancement validé peut redevenir un brouillon (et seulement s'il n'y a pas déjà un brouillon).
async fn remettre_en_brouillon(State(state): State<AppState>, Path((chantier_id, id)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    let a = charger_avancement(&state.db, chantier_id, id).await?;
    if a.statut != "valide" || a.numero != a.dernier {
        return Err(Erreur::conflit("Seul le dernier avancement validé peut être remis en brouillon."));
    }
    sqlx::query("UPDATE avancements SET statut = 'brouillon', updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Redirect::to(&format!("/chantiers/{chantier_id}/avancements/{id}?ok=brouillon")))
}

/// Supprimer un brouillon (c'est toujours le dernier avancement : la numérotation ne garde pas de trou).
async fn supprimer(State(state): State<AppState>, Path((chantier_id, id)): Path<(i64, i64)>) -> Result<Redirect, Erreur> {
    let a = charger_avancement(&state.db, chantier_id, id).await?;
    if a.statut != "brouillon" {
        return Err(Erreur::conflit("Un avancement validé ne se supprime pas."));
    }
    sqlx::query("DELETE FROM avancements WHERE id = $1 AND statut = 'brouillon'").bind(id).execute(&state.db).await?;
    Ok(Redirect::to(&format!("/chantiers/{chantier_id}/finances")))
}

// ─── L'onglet Finances du chantier ──────────────────────────────────────────

pub struct LigneListe {
    pub id: i64,
    pub numero: i32,
    pub date: String,
    pub reference: String,
    pub montant: String,
    pub pourcentage: String,
    pub pourcentage_cumul: String,
    pub reste: String,
    pub brouillon: bool,
}

pub struct Finances {
    pub avancements: Vec<LigneListe>,
    /// L'avancement en brouillon, s'il y en a un : « Nouvel avancement » le rouvre.
    pub brouillon: Option<i64>,
    pub total: String,
    pub avance: String,
    pub pourcentage_avance: String,
    pub reste: String,
}

pub async fn finances(db: &PgPool, chantier_id: i64) -> Result<Finances, sqlx::Error> {
    let total: i64 = charger_blocs(db, chantier_id).await?.iter().map(|b| b.total_cents).sum();
    let lignes: Vec<(i64, i32, NaiveDate, String, i64, String)> = sqlx::query_as(
        "SELECT id, numero, date_avancement, reference, montant_ht_cents, statut::text FROM avancements WHERE chantier_id = $1 ORDER BY numero",
    )
    .bind(chantier_id)
    .fetch_all(db)
    .await?;
    let mut cumul = 0;
    let mut avance = 0;
    let mut brouillon = None;
    let mut vues = Vec::new();
    for (id, numero, date, reference, montant, statut) in lignes {
        cumul += montant;
        if statut == "brouillon" {
            brouillon = Some(id);
        } else {
            avance += montant;
        }
        let pourcent = |m: i64| ac::pourcentage_centiemes(m, total).map(ac::pourcentage_en_texte).unwrap_or_default();
        vues.push(LigneListe {
            id,
            numero,
            date: date.format("%d/%m/%Y").to_string(),
            reference,
            montant: format_euros(montant),
            pourcentage: pourcent(montant),
            pourcentage_cumul: pourcent(cumul),
            reste: format_euros(total - cumul),
            brouillon: statut == "brouillon",
        });
    }
    Ok(Finances {
        avancements: vues,
        brouillon,
        total: format_euros(total),
        avance: format_euros(avance),
        pourcentage_avance: ac::pourcentage_centiemes(avance, total).map(ac::pourcentage_en_texte).unwrap_or_default(),
        reste: format_euros(total - avance),
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/chantiers/{id}/avancements", post(creer))
        .route("/chantiers/{id}/avancements/{avancement}", get(afficher))
        .route("/chantiers/{id}/avancements/{avancement}/infos", post(enregistrer_infos))
        .route("/chantiers/{id}/avancements/{avancement}/lignes/{cle}", post(saisir))
        .route("/chantiers/{id}/avancements/{avancement}/valider", post(valider))
        .route("/chantiers/{id}/avancements/{avancement}/brouillon", post(remettre_en_brouillon))
        .route("/chantiers/{id}/avancements/{avancement}/supprimer", post(supprimer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ligne(id: i64, genre: &str, q: Option<i64>, p: Option<i64>) -> LigneDevis {
        LigneDevis {
            id,
            devis_id: 1,
            genre: genre.to_string(),
            description: format!("L{id}"),
            quantite_e5: q,
            unite: String::new(),
            prix_unitaire_e5: p,
            taux_tva_id: Some(1),
        }
    }

    fn devis(montant: i64) -> DevisDuChantier {
        DevisDuChantier { id: 1, titre: "Devis 2026-57".to_string(), montant_ht_cents: montant, ajustement_libelle: String::new() }
    }

    #[test]
    fn les_lignes_sont_numerotees_comme_dans_le_devis() {
        let lignes = [
            ligne(1, "titre", None, None),
            ligne(2, "poste_libre", Some(100_000), Some(1_000_000)),
            ligne(3, "saut_de_page", None, None),
            ligne(4, "texte", None, None),
            ligne(5, "titre", None, None),
            ligne(6, "fourniture", Some(200_000), Some(500_000)),
        ];
        let refs: Vec<&LigneDevis> = lignes.iter().collect();
        let b = construire_bloc(&devis(2_000), &refs);
        let numeros: Vec<&str> = b.postes.iter().map(|p| p.numero.as_str()).collect();
        assert_eq!(numeros, vec!["1.0", "1.1", "1.2", "2.0", "2.1"]);
        let avancables: Vec<&str> = b.postes.iter().filter(|p| p.avancable()).map(|p| p.cle.as_str()).collect();
        assert_eq!(avancables, vec!["l2", "l6"]);
    }

    #[test]
    fn l_ajustement_du_devis_devient_un_poste() {
        // Lignes : 100 € ; devis après remise : 90 € → un poste « Ajustement » de −10 €.
        let lignes = [ligne(2, "poste_libre", Some(100_000), Some(10_000_000))];
        let refs: Vec<&LigneDevis> = lignes.iter().collect();
        let b = construire_bloc(&devis(9_000), &refs);
        let aj = b.postes.last().unwrap();
        assert_eq!((aj.cle.as_str(), aj.nature, aj.total_cents, aj.description.as_str()), ("a1", Nature::Ajustement, -1_000, "Ajustement"));
        assert_eq!(b.postes.iter().map(|p| p.total_cents).sum::<i64>(), b.total_cents);
    }

    #[test]
    fn precedent_et_periode() {
        let montants = vec![(1, "l2".to_string(), 100), (2, "l2".to_string(), 50), (2, "l6".to_string(), 7), (3, "l2".to_string(), 1)];
        let (prec, per) = repartir(&montants, 2);
        assert_eq!(prec.get("l2"), Some(&100));
        assert_eq!(per.get("l2"), Some(&50));
        assert_eq!(per.get("l6"), Some(&7));
        assert_eq!(prec.get("l6"), None);
    }

    #[test]
    fn les_cles_de_poste() {
        assert_eq!(colonnes_cle("l12"), (Some(12), None));
        assert_eq!(colonnes_cle("a5"), (None, Some(5)));
        assert_eq!(colonnes_cle("x1"), (None, None));
        assert_eq!(colonnes_cle(""), (None, None));
    }
}
