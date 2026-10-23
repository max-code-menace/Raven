//! Éditeur de devis, onglet Encodage (étape 5b-2) : titres, sous-titres, textes et postes libres.
//! Chaque changement est enregistré tout de suite ; le serveur recalcule les totaux.

use askama::Template;
use axum::{
    Router,
    extract::{Form as FormSimple, Path},
    response::Html,
    routing::{get, post},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::{FromRow, PgConnection, Postgres, Transaction};

use crate::base::Base;
use crate::calcul::{self, Ajustement, Totaux};
use crate::commun::{Choix, format_euros};
use crate::devis_statut::{Barre, charger_barre};
use crate::fiches::{Genre, Options};
use crate::{AppState, Erreur};

/// Les types de ligne, dans l'ordre du menu « Ajouter ».
const GENRES: [(&str, &str); 10] = [
    ("titre", "Titre"),
    ("sous_titre", "Sous-titre"),
    ("texte", "Texte"),
    ("poste_libre", "Poste libre"),
    ("ouvrage", "Ouvrage"),
    ("fourniture", "Fourniture"),
    ("equipement", "Équipements"),
    ("main_oeuvre", "Main d'œuvre"),
    ("sous_traitant", "Sous-traitant"),
    ("saut_de_page", "Saut de page"),
];
/// Les types qui portent une quantité, un prix et une TVA.
const GENRES_CHIFFRES: [&str; 6] = ["poste_libre", "ouvrage", "fourniture", "equipement", "main_oeuvre", "sous_traitant"];
/// Types de quantité d'une ligne chiffrée (fiche QuoteTree) : (valeur, sigle, mot entier, explication).
/// Il ne change aucun calcul : il dit au client comment la quantité sera facturée.
pub(crate) const TYPES_QUANTITE: [(&str, &str, &str, &str); 3] = [
    ("fixe", "QF", "Quantité fixe", "La quantité du devis est celle qui sera facturée."),
    ("estimee", "QP", "Quantité estimée", "Quantité estimée, facturée selon le métré réel."),
    ("forfait", "Fft", "Forfait", "Un prix global, quelle que soit la quantité."),
];
const LONGUEUR_MAX_REMARQUE: usize = 2_000;
const LONGUEUR_MAX_DESCRIPTION: usize = 5_000;
const LONGUEUR_MAX_UNITE: usize = 20;
const MESSAGE_NON_MODIFIABLE: &str = "Ce devis n'est plus un brouillon : il ne peut plus être modifié.";

// ─── Ce que l'écran affiche ─────────────────────────────────────────────────

/// Ce que les onglets du devis ont en commun : numéro, statut, TVA par défaut et ajustement.
pub(crate) struct Entete {
    /// « 2026-57 » pour un devis, « 2 » pour un avenant (numéroté dans son chantier).
    pub(crate) numero: String,
    /// « Devis » ou « Avenant ».
    pub(crate) genre: &'static str,
    pub(crate) brouillon: bool,
    pub(crate) taux_par_defaut: Option<i64>,
    pub(crate) ajustement: Ajustement,
    pub(crate) ajustement_libelle: String,
    /// L'option « marge et prix d'achat » est-elle allumée ?
    pub(crate) marge_activee: bool,
    /// Frais généraux en centièmes de pour cent des achats (1000 = 10 %).
    pub(crate) frais_generaux_centiemes: i64,
}

/// Un devis s'affiche « année-numéro » ; un avenant garde son numéro dans le chantier (« 2 »).
pub(crate) fn numero_affiche(annee: i32, numero: i64, avenant: bool) -> String {
    if avenant { numero.to_string() } else { format!("{annee}-{numero}") }
}

pub(crate) async fn charger_entete(db: &mut PgConnection, id: i64) -> Result<Entete, Erreur> {
    let ligne: Option<(i64, i32, String, Option<i64>, String, String, i64, bool, i32, bool)> = sqlx::query_as(
        "SELECT numero, extract(year FROM date_devis)::int, statut::text, taux_tva_id,
                ajustement_libelle, ajustement_mode, ajustement_valeur, marge_activee, frais_generaux_centiemes,
                chantier_id IS NOT NULL
         FROM devis WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *db)
    .await?;
    let (numero, annee, statut_devis, taux_par_defaut, ajustement_libelle, mode, valeur, marge_activee, frais, avenant) =
        ligne.ok_or_else(Erreur::introuvable)?;
    Ok(Entete {
        numero: numero_affiche(annee, numero, avenant),
        genre: if avenant { "Avenant" } else { "Devis" },
        brouillon: statut_devis == "brouillon",
        taux_par_defaut,
        ajustement: Ajustement::depuis_base(&mode, valeur),
        ajustement_libelle,
        marge_activee,
        frais_generaux_centiemes: i64::from(frais),
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
    prix_achat_unitaire_e5: Option<i64>,
    remarque: String,
    taux_tva_id: Option<i64>,
    taux_centiemes: Option<i32>,
    type_quantite: String,
}

impl LigneBase {
    /// Un type qui porte une quantité, un prix et une TVA (par opposition à un titre ou un texte).
    fn est_chiffre(&self) -> bool {
        GENRES_CHIFFRES.contains(&self.genre.as_str())
    }

    /// Total HT de la ligne (0 tant que la quantité ou le prix manque).
    fn total_cents(&self) -> i64 {
        match (self.quantite_e5, self.prix_unitaire_e5) {
            (Some(q), Some(p)) if self.est_chiffre() => calcul::total_ligne_cents(q, p),
            _ => 0,
        }
    }

    /// Quantité × prix d'achat (0 tant que l'un des deux manque).
    fn achat_total_cents(&self) -> i64 {
        match (self.quantite_e5, self.prix_achat_unitaire_e5) {
            (Some(q), Some(p)) if self.est_chiffre() => calcul::total_ligne_cents(q, p),
            _ => 0,
        }
    }

    fn en_rentable(&self) -> Option<calcul::LigneRentable> {
        let categorie = calcul::Categorie::depuis_genre(&self.genre)?;
        let heures = match (categorie, self.quantite_e5) {
            (calcul::Categorie::MainOeuvre, Some(q)) if calcul::est_une_heure(&self.unite) => q,
            _ => 0,
        };
        Some(calcul::LigneRentable {
            categorie,
            ventes_cents: self.total_cents(),
            achats_cents: self.achat_total_cents(),
            heures_e5: heures,
            a_un_prix_d_achat: self.quantite_e5.is_some() && self.prix_achat_unitaire_e5.is_some(),
        })
    }
}

/// Les colonnes d'une ligne, dans l'ordre de `LigneBase` (avec la TVA jointe sous le nom `t`).
const COLONNES_LIGNE: &str = "l.id, l.genre::text AS genre, l.description, l.quantite_e5, l.unite, l.prix_unitaire_e5,
                l.prix_achat_unitaire_e5, l.remarque, l.taux_tva_id, l.type_quantite::text AS type_quantite";

async fn charger_lignes<'e>(db: impl sqlx::PgExecutor<'e>, devis_id: i64) -> Result<Vec<LigneBase>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLONNES_LIGNE}, t.taux_centiemes
         FROM devis_lignes l LEFT JOIN taux_tva t ON t.id = l.taux_tva_id
         WHERE l.devis_id = $1 ORDER BY l.position",
    )))
    .bind(devis_id)
    .fetch_all(db)
    .await
}

/// Une mesure du métré d'une ligne (étape 4-2) : la somme des mesures est la quantité de la ligne.
#[derive(FromRow)]
pub(crate) struct Mesure {
    pub(crate) id: i64,
    pub(crate) ligne_id: i64,
    pub(crate) libelle: String,
    pub(crate) longueur_e5: i64,
    pub(crate) largeur_e5: Option<i64>,
    pub(crate) deduction: bool,
}

impl Mesure {
    pub(crate) fn resultat_e5(&self) -> i64 {
        calcul::mesure_e5(self.longueur_e5, self.largeur_e5, self.deduction)
    }
}

/// Les mesures de toutes les lignes d'un devis, dans l'ordre.
async fn charger_mesures<'e>(db: impl sqlx::PgExecutor<'e>, devis_id: i64) -> Result<Vec<Mesure>, sqlx::Error> {
    sqlx::query_as(
        "SELECT m.id, m.ligne_id, m.libelle, m.longueur_e5, m.largeur_e5, m.deduction
         FROM devis_metres m JOIN devis_lignes l ON l.id = m.ligne_id
         WHERE l.devis_id = $1 ORDER BY m.ligne_id, m.position, m.id",
    )
    .bind(devis_id)
    .fetch_all(db)
    .await
}

/// Les unités proposées (liste de suggestions, l'unité reste libre) selon le métier de l'organisation.
pub(crate) fn unites(metier: &str) -> &'static [&'static str] {
    match metier {
        "batiment" => &["m²", "m", "m³", "pièce", "h", "jour", "kg", "tonne", "litre", "sac", "forfait"],
        "espaces_verts" => &["m²", "m", "m³", "pièce", "stère", "tonne", "h", "jour", "forfait"],
        "peinture" => &["m²", "m", "pièce", "litre", "h", "jour", "forfait"],
        "graphisme" => &["h", "jour", "page", "pièce", "forfait"],
        _ => &["h", "jour", "pièce", "m²", "m", "forfait"],
    }
}

/// Totaux du devis à partir de ses lignes. Une ligne sans taux de TVA n'est pas comptée
/// (le serveur ne laisse pas créer de ligne chiffrée sans taux).
fn totaux_des_lignes(lignes: &[LigneBase], ajustement: Ajustement) -> Totaux {
    let comptees: Vec<(i64, i32)> =
        lignes.iter().filter(|l| l.est_chiffre()).filter_map(|l| Some((l.total_cents(), l.taux_centiemes?))).collect();
    calcul::totaux(&comptees, ajustement)
}

pub struct LigneVue {
    pub id: i64,
    pub genre: &'static str,
    pub libelle_genre: &'static str,
    pub est_titre: bool,
    /// Type chiffré : quantité, unité, prix, TVA et total.
    pub est_chiffre: bool,
    pub est_saut: bool,
    pub numero: String,
    pub description: String,
    pub remarque: String,
    pub quantite: String,
    pub unite: String,
    pub prix_unitaire: String,
    /// Prix d'achat unitaire à saisir (option marge).
    pub prix_achat: String,
    /// « 20,00 € (20 %) » : bénéfice de la ligne et sa marge sur le prix de vente ; vide sans prix d'achat.
    pub marge: String,
    /// Coefficient de vente (prix de vente ÷ prix d'achat, « 1,25 ») ; vide sans prix d'achat.
    pub coefficient: String,
    pub taux: Vec<Choix>,
    /// Taux de TVA de la ligne, pour l'affichage en lecture (« 6 % »).
    pub taux_texte: String,
    /// Prix unitaire pour l'affichage en lecture (« 50,00 € »).
    pub prix_affiche: String,
    pub total: String,
    pub sous_total: String,
    pub premiere: bool,
    pub derniere: bool,
    /// « fixe », « estimee » ou « forfait » (voir `TYPES_QUANTITE`).
    pub type_quantite: &'static str,
    /// Le sigle affiché en lecture (« QF »).
    pub sigle_quantite: &'static str,
    /// La quantité vient du métré (au moins une mesure).
    pub a_metre: bool,
    /// Le métré est déplié sous la ligne.
    pub metre_ouvert: bool,
    pub mesures: Vec<MesureVue>,
}

pub struct MesureVue {
    pub id: i64,
    pub libelle: String,
    pub longueur: String,
    pub largeur: String,
    pub deduction: bool,
    /// « 6,2 », « -0,35 ».
    pub resultat: String,
}

pub struct TvaVue {
    pub taux: String,
    pub base: String,
    pub montant: String,
}

/// Les totaux du devis, prêts à afficher.
pub struct TotauxVue {
    pub sous_total_cents: i64,
    pub sous_total: String,
    pub a_ajustement: bool,
    pub libelle_ajustement: String,
    pub ajustement: String,
    pub ht: String,
    pub tva: Vec<TvaVue>,
    pub ttc: String,
}

pub(crate) fn vue_totaux(t: &Totaux, libelle_ajustement: &str) -> TotauxVue {
    TotauxVue {
        sous_total_cents: t.sous_total_cents,
        sous_total: format_euros(t.sous_total_cents),
        a_ajustement: t.ajustement_cents != 0,
        libelle_ajustement: if libelle_ajustement.is_empty() { "Ajustement".to_string() } else { libelle_ajustement.to_string() },
        ajustement: format_euros(t.ajustement_cents),
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
    }
}

/// Une ligne du tableau de rentabilité (une catégorie, ou le total), prête à afficher.
pub struct RentableVue {
    pub libelle: String,
    pub achats: String,
    pub frais: String,
    pub marge: String,
    /// La marge sur le prix de vente (« 34 % »), ou « — » sans vente.
    pub taux_marge: String,
    pub ventes: String,
    /// « 16 h », ou « — » sans heures.
    pub heures: String,
}

pub struct RentabiliteVue {
    pub categories: Vec<RentableVue>,
    pub a_ajustement: bool,
    pub ajustement: String,
    pub total: RentableVue,
    pub lignes_sans_achat: usize,
}

fn vue_rentable(libelle: &str, r: &calcul::Rentable) -> RentableVue {
    RentableVue {
        libelle: libelle.to_string(),
        achats: format_euros(r.achats_cents),
        frais: format_euros(r.frais_generaux_cents),
        marge: format_euros(r.marge_cents),
        taux_marge: r.taux_marge_centiemes().map(|t| calcul::taux_en_texte(t as i32)).unwrap_or_else(|| "—".to_string()),
        ventes: format_euros(r.ventes_cents),
        heures: if r.heures_e5 == 0 { "—".to_string() } else { format!("{} h", calcul::ecrire_decimal_e5(r.heures_e5)) },
    }
}

fn vue_rentabilite(r: &calcul::Rentabilite) -> RentabiliteVue {
    RentabiliteVue {
        categories: r.categories.iter().map(|(c, v)| vue_rentable(crate::profil::categorie(c.libelle()), v)).collect(),
        a_ajustement: r.ajustement_cents != 0,
        ajustement: format_euros(r.ajustement_cents),
        total: vue_rentable("Total", &r.total),
        lignes_sans_achat: r.lignes_sans_achat,
    }
}

pub struct Encodage {
    pub id: i64,
    pub modifiable: bool,
    pub lignes: Vec<LigneVue>,
    pub totaux: TotauxVue,
    /// Message affiché au-dessus du tableau (erreur de saisie, par exemple).
    pub message: String,
    /// Les types de ligne du menu « Ajouter » : (valeur, libellé).
    pub types: Vec<(&'static str, &'static str)>,
    /// L'option « marge et prix d'achat » est allumée.
    pub marge_activee: bool,
    /// Frais généraux à saisir, en pour cent des achats (« 10 », « 7,5 »).
    pub frais_generaux: String,
    /// Colonnes que prend la description d'une ligne non chiffrée : description, quantité, unité,
    /// prix, TVA (et le prix d'achat quand l'option marge est allumée).
    pub colonnes_description: usize,
    /// Présente seulement quand l'option marge est allumée.
    pub rentabilite: Option<RentabiliteVue>,
    /// Bénéfice du devis en pour cent de son prix de vente (« 20 % », « — » sans vente).
    pub marge_sur_vente: String,
    /// Ventes ÷ achats des lignes qui ont un prix d'achat (« 1,25 », « — » sans achat).
    pub coefficient_moyen: String,
    /// La ligne dont le prix de vente vient d'être calculé par son coefficient : son champ est renvoyé à htmx.
    pub prix_change: Option<i64>,
    /// Retour visuel (audit du design du 23/10/2026) : la ligne dont le total vient de changer, la ligne qui vient
    /// d'être ajoutée, et si les totaux du devis ont changé. Ce qui a changé s'éclaire en jaune puis s'efface.
    pub ligne_changee: Option<i64>,
    pub ligne_ajoutee: Option<i64>,
    pub totaux_changes: bool,
    /// Les types de quantité (valeur, sigle, mot entier, explication).
    pub types_quantite: [(&'static str, &'static str, &'static str, &'static str); 3],
    /// Les unités proposées pour le métier de l'organisation.
    pub unites: &'static [&'static str],
}

fn type_quantite_connu(valeur: &str) -> Option<(&'static str, &'static str)> {
    TYPES_QUANTITE.iter().find(|t| t.0 == valeur).map(|t| (t.0, t.1))
}

fn genre_connu(genre: &str) -> Option<(&'static str, &'static str)> {
    GENRES.iter().find(|(v, _)| *v == genre).copied()
}

/// Ce que l'encodage montre en plus des lignes : leurs mesures, la ligne dont le métré est déplié, les unités.
struct Contexte<'a> {
    mesures: &'a [Mesure],
    metre_ouvert: Option<i64>,
    unites: &'static [&'static str],
}

fn construire_encodage(
    id: i64,
    modifiable: bool,
    lignes: &[LigneBase],
    taux: &[(i64, String)],
    entete: &Entete,
    message: String,
    contexte: Contexte<'_>,
) -> Encodage {
    // Numérotation : un titre ouvre un lot (1.0, 2.0…), les lignes suivantes en font partie (1.1, 1.2…).
    // Le sous-total d'un titre est la somme des postes jusqu'au titre suivant.
    let mut vues = Vec::with_capacity(lignes.len());
    let (mut lot, mut rang) = (0, 0);
    let mut sous_totaux: Vec<(usize, i64)> = Vec::new();
    for (i, l) in lignes.iter().enumerate() {
        let (libelle_genre, genre) = match genre_connu(&l.genre) {
            Some((v, lib)) => (crate::profil::type_ligne(lib), v),
            None => ("?", "texte"),
        };
        let est_titre = l.genre == "titre";
        let est_saut = l.genre == "saut_de_page";
        let numero = if est_titre {
            lot += 1;
            rang = 0;
            sous_totaux.push((i, 0));
            format!("{lot}.0")
        } else if est_saut {
            // Un saut de page n'est pas une ligne du devis : pas de numéro.
            String::new()
        } else {
            rang += 1;
            if let Some((_, somme)) = sous_totaux.last_mut() {
                *somme += l.total_cents();
            }
            format!("{lot}.{rang}")
        };
        // Bénéfice de la ligne : vente − achat, et sa marge sur le prix de vente. Vide sans prix d'achat.
        let marge = match (l.est_chiffre(), l.quantite_e5, l.prix_achat_unitaire_e5) {
            (true, Some(_), Some(_)) => {
                let (vente, achat) = (l.total_cents(), l.achat_total_cents());
                let taux = calcul::taux_sur_vente(vente - achat, vente).map(|t| format!(" ({})", calcul::taux_en_texte(t as i32)));
                format!("{}{}", format_euros(vente - achat), taux.unwrap_or_default())
            }
            _ => String::new(),
        };
        let (type_quantite, sigle_quantite) = type_quantite_connu(&l.type_quantite).unwrap_or(("fixe", "QF"));
        let mesures: Vec<MesureVue> = contexte
            .mesures
            .iter()
            .filter(|m| m.ligne_id == l.id)
            .map(|m| MesureVue {
                id: m.id,
                libelle: m.libelle.clone(),
                longueur: calcul::ecrire_decimal_e5(m.longueur_e5),
                largeur: m.largeur_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
                deduction: m.deduction,
                resultat: calcul::ecrire_decimal_e5(m.resultat_e5()),
            })
            .collect();
        let a_metre = l.est_chiffre() && !mesures.is_empty();
        vues.push(LigneVue {
            id: l.id,
            genre,
            libelle_genre,
            est_titre,
            est_chiffre: l.est_chiffre(),
            est_saut,
            numero,
            description: l.description.clone(),
            remarque: l.remarque.clone(),
            quantite: l.quantite_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
            unite: l.unite.clone(),
            prix_unitaire: l.prix_unitaire_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
            prix_achat: l.prix_achat_unitaire_e5.map(calcul::ecrire_decimal_e5).unwrap_or_default(),
            marge,
            coefficient: match (l.est_chiffre(), l.prix_unitaire_e5, l.prix_achat_unitaire_e5) {
                (true, Some(v), Some(a)) => calcul::coefficient_e5(v, a).map(calcul::ecrire_decimal_e5).unwrap_or_default(),
                _ => String::new(),
            },
            taux: taux
                .iter()
                .map(|(tid, libelle)| Choix {
                    valeur: tid.to_string(),
                    libelle: libelle.clone(),
                    choisi: Some(*tid) == l.taux_tva_id,
                })
                .collect(),
            taux_texte: l.taux_centiemes.map(calcul::taux_en_texte).unwrap_or_default(),
            prix_affiche: l.prix_unitaire_e5.map(calcul::prix_pour_affichage).unwrap_or_default(),
            total: if l.est_chiffre() { format_euros(l.total_cents()) } else { String::new() },
            sous_total: String::new(),
            premiere: i == 0,
            derniere: i + 1 == lignes.len(),
            type_quantite,
            sigle_quantite,
            a_metre,
            metre_ouvert: l.est_chiffre() && contexte.metre_ouvert == Some(l.id),
            mesures,
        });
    }
    for (i, somme) in sous_totaux {
        vues[i].sous_total = format_euros(somme);
    }

    let t = totaux_des_lignes(lignes, entete.ajustement);
    let rentables: Vec<calcul::LigneRentable> = lignes.iter().filter_map(LigneBase::en_rentable).collect();
    let calculee = calcul::rentabilite(&rentables, entete.frais_generaux_centiemes, t.ajustement_cents);
    // Marge du devis sur son prix de vente (frais généraux et remise compris).
    let marge_sur_vente = calcul::taux_sur_vente(calculee.total.marge_cents, calculee.total.ventes_cents)
        .map(|t| calcul::taux_en_texte(t as i32))
        .unwrap_or_else(|| "—".to_string());
    // Coefficient moyen : ventes ÷ achats des seules lignes qui ont un prix d'achat.
    let (ventes_avec_achat, achats): (i64, i64) = rentables
        .iter()
        .filter(|r| r.a_un_prix_d_achat)
        .fold((0, 0), |(v, a), r| (v + r.ventes_cents, a + r.achats_cents));
    let coefficient_moyen = calcul::coefficient_e5(ventes_avec_achat, achats).map(calcul::ecrire_decimal_e5).unwrap_or_else(|| "—".to_string());
    let rentabilite = entete.marge_activee.then(|| vue_rentabilite(&calculee));
    Encodage {
        id,
        modifiable,
        lignes: vues,
        totaux: vue_totaux(&t, &entete.ajustement_libelle),
        message,
        types: GENRES.iter().map(|&(v, l)| (v, crate::profil::type_ligne(l))).collect(),
        marge_activee: entete.marge_activee,
        // centièmes de pour cent → texte en pour cent : × 1000 pour passer en 1/100 000
        frais_generaux: calcul::ecrire_decimal_e5(entete.frais_generaux_centiemes * 1000),
        colonnes_description: if entete.marge_activee { 6 } else { 5 },
        rentabilite,
        marge_sur_vente,
        coefficient_moyen,
        prix_change: None,
        ligne_changee: None,
        ligne_ajoutee: None,
        totaux_changes: false,
        types_quantite: TYPES_QUANTITE,
        unites: contexte.unites,
    }
}

pub(crate) async fn charger_encodage(db: &mut PgConnection, id: i64, entete: &Entete, message: String) -> Result<Encodage, Erreur> {
    charger_encodage_metre(db, id, entete, message, None).await
}

/// L'encodage avec le métré de la ligne `metre_ouvert` déplié.
pub(crate) async fn charger_encodage_metre(
    db: &mut PgConnection,
    id: i64,
    entete: &Entete,
    message: String,
    metre_ouvert: Option<i64>,
) -> Result<Encodage, Erreur> {
    let lignes = charger_lignes(&mut *db, id).await?;
    let mesures = charger_mesures(&mut *db, id).await?;
    let options = Options::charger(&mut *db, Genre::Entreprise).await?;
    let metier: String =
        sqlx::query_scalar("SELECT profil_metier FROM organisations WHERE id = app_organisation()").fetch_one(&mut *db).await?;
    let contexte = Contexte { mesures: &mesures, metre_ouvert, unites: unites(&metier) };
    Ok(construire_encodage(id, entete.brouillon, &lignes, &crate::fiches::taux_courts(&options.taux_tva), entete, message, contexte))
}

// ─── Templates ──────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "devis_encodage.html")]
struct Page {
    id: i64,
    onglet: &'static str,
    barre: Barre,
    enc: Encodage,
    /// Après un import Excel : « 24 lignes ajoutées… ».
    message_ok: String,
}

/// Tout le bloc d'encodage : renvoyé à htmx après un ajout, une suppression ou un déplacement.
#[derive(Template)]
#[template(path = "_encodage.html")]
pub(crate) struct Bloc {
    pub(crate) enc: Encodage,
}

/// Seulement les totaux et les messages : renvoyé après la modification d'une ligne, pour ne pas
/// toucher aux champs en cours de saisie.
#[derive(Template)]
#[template(path = "_encodage_maj.html")]
pub(crate) struct Maj {
    pub(crate) enc: Encodage,
}

// ─── Modifications ──────────────────────────────────────────────────────────

pub(crate) fn exiger_brouillon(entete: &Entete) -> Result<(), Erreur> {
    if entete.brouillon { Ok(()) } else { Err(Erreur::conflit(MESSAGE_NON_MODIFIABLE)) }
}

/// Recalcule et enregistre les totaux du devis dans la même transaction que le changement.
/// La rentabilité prévue d'un devis ou d'un avenant (suivi de chantier, étape 9-2) : ventes, achats, frais généraux,
/// marge et heures. L'ajustement est la différence entre le total HT enregistré et la somme des lignes.
pub(crate) async fn rentabilite_prevue(db: &mut PgConnection, devis_id: i64) -> Result<calcul::Rentabilite, sqlx::Error> {
    let lignes = charger_lignes(&mut *db, devis_id).await?;
    let (montant_ht, frais_generaux): (i64, i32) =
        sqlx::query_as("SELECT montant_ht_cents, frais_generaux_centiemes FROM devis WHERE id = $1").bind(devis_id).fetch_one(&mut *db).await?;
    let rentables: Vec<calcul::LigneRentable> = lignes.iter().filter_map(LigneBase::en_rentable).collect();
    let somme: i64 = rentables.iter().map(|l| l.ventes_cents).sum();
    Ok(calcul::rentabilite(&rentables, i64::from(frais_generaux), montant_ht - somme))
}

pub(crate) async fn recalculer(tx: &mut Transaction<'_, Postgres>, devis_id: i64) -> Result<(), sqlx::Error> {
    let lignes = charger_lignes(&mut **tx, devis_id).await?;
    let (mode, valeur): (String, i64) = sqlx::query_as("SELECT ajustement_mode, ajustement_valeur FROM devis WHERE id = $1")
        .bind(devis_id)
        .fetch_one(&mut **tx)
        .await?;
    let t = totaux_des_lignes(&lignes, Ajustement::depuis_base(&mode, valeur));
    sqlx::query("UPDATE devis SET montant_ht_cents = $1, montant_ttc_cents = $2, updated_at = now() WHERE id = $3")
        .bind(t.ht_cents)
        .bind(t.ttc_cents)
        .bind(devis_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RetourEncodage {
    /// Le nombre de lignes importées depuis Excel.
    import: String,
    /// La ligne dont le métré est déplié.
    metre: String,
}

async fn page(
    mut db: Base,
    Path(id): Path<i64>,
    entetes: axum::http::HeaderMap,
    axum::extract::Query(r): axum::extract::Query<RetourEncodage>,
) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    let enc = charger_encodage_metre(&mut *db, id, &entete, String::new(), r.metre.parse().ok()).await?;
    // Déplier ou replier un métré (htmx) : seulement le bloc d'encodage.
    if entetes.contains_key("hx-request") && !entetes.contains_key("hx-history-restore-request") {
        return Ok(Html(Bloc { enc }.render()?));
    }
    let barre = charger_barre(&mut *db, id).await?;
    let message_ok = match r.import.parse::<u32>() {
        Ok(n) => format!("Import Excel terminé : {}. Vérifiez les totaux ci-dessous.", crate::commun::nombre(n, "ligne ajoutée", "lignes ajoutées")),
        Err(_) => String::new(),
    };
    let page = Page { id, onglet: "encodage", barre, enc, message_ok };
    Ok(Html(page.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NouvelleLigne {
    genre: String,
}

async fn ajouter(mut db: Base, Path(id): Path<i64>, Form(f): Form<NouvelleLigne>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let Some((genre, _)) = genre_connu(&f.genre) else {
        let enc = charger_encodage(&mut *db, id, &entete, "Ce type de ligne n'existe pas.".to_string()).await?;
        return Ok(Html(Bloc { enc }.render()?));
    };
    let est_chiffre = GENRES_CHIFFRES.contains(&genre);
    if est_chiffre && entete.taux_par_defaut.is_none() {
        let message = "Choisissez d'abord le taux de TVA par défaut dans l'onglet Informations.".to_string();
        let enc = charger_encodage(&mut *db, id, &entete, message).await?;
        return Ok(Html(Bloc { enc }.render()?));
    }

    let mut tx = db.begin().await?;
    // Verrouille le devis : deux ajouts en même temps ne prennent pas la même position.
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    // Une ligne chiffrée naît avec la quantité 1 et la TVA par défaut du devis ; la main d'œuvre en heures.
    let nouvelle: i64 = sqlx::query_scalar(
        "INSERT INTO devis_lignes (devis_id, position, genre, quantite_e5, taux_tva_id, unite)
         VALUES ($1, COALESCE((SELECT max(position) FROM devis_lignes WHERE devis_id = $1), 0) + 1,
                 $2::text::genre_ligne, $3, $4, $5)
         RETURNING id",
    )
    .bind(id)
    .bind(genre)
    .bind(est_chiffre.then_some(100_000_i64))
    .bind(if est_chiffre { entete.taux_par_defaut } else { None })
    .bind(if genre == "main_oeuvre" { "h" } else { "" })
    .fetch_one(&mut *tx)
    .await?;
    recalculer(&mut tx, id).await?;
    tx.commit().await?;

    let mut enc = charger_encodage(&mut *db, id, &entete, String::new()).await?;
    enc.ligne_ajoutee = Some(nouvelle);
    Ok(Html(Bloc { enc }.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OuvrageChoisi {
    ouvrage: i64,
}

/// Insère un ouvrage de la bibliothèque (étape 9-3) à la fin du devis : une copie de sa description, de son type, de
/// son unité, de ses prix d'achat et de vente et de sa remarque, quantité 1, TVA par défaut du devis.
async fn ajouter_ouvrage(mut db: Base, Path(id): Path<i64>, Form(f): Form<OuvrageChoisi>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    if entete.taux_par_defaut.is_none() {
        let message = "Choisissez d'abord le taux de TVA par défaut dans l'onglet Informations.".to_string();
        let enc = charger_encodage(&mut *db, id, &entete, message).await?;
        return Ok(Html(Bloc { enc }.render()?));
    }
    let mut tx = db.begin().await?;
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    // Un ouvrage sans prix de vente (« à compléter ») arrive à 0 € : le message le signale pour qu'on le complète.
    let insere: Option<(i64, bool)> = sqlx::query_as(
        "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5,
                                   remarque, taux_tva_id)
         SELECT $1, COALESCE((SELECT max(position) FROM devis_lignes WHERE devis_id = $1), 0) + 1,
                genre, description, 100000, unite, COALESCE(prix_vente_e5, 0), prix_achat_e5, remarque, $3
         FROM ouvrages WHERE id = $2 AND supprime_le IS NULL
         RETURNING id, (SELECT prix_vente_e5 IS NULL FROM ouvrages WHERE id = $2)",
    )
    .bind(id)
    .bind(f.ouvrage)
    .bind(entete.taux_par_defaut)
    .fetch_optional(&mut *tx)
    .await?;
    let message = match insere {
        None => "Cet ouvrage n'existe plus dans la bibliothèque.".to_string(),
        Some((_, true)) => "Cet ouvrage n'a pas encore de prix dans votre bibliothèque : la ligne est ajoutée à 0 €, indiquez son prix.".to_string(),
        Some((_, false)) => String::new(),
    };
    recalculer(&mut tx, id).await?;
    tx.commit().await?;
    let mut enc = charger_encodage(&mut *db, id, &entete, message).await?;
    enc.ligne_ajoutee = insere.map(|(ligne, _)| ligne);
    enc.totaux_changes = insere.is_some();
    Ok(Html(Bloc { enc }.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MajLigne {
    description: Option<String>,
    remarque: Option<String>,
    quantite: Option<String>,
    unite: Option<String>,
    prix_unitaire: Option<String>,
    prix_achat: Option<String>,
    taux_tva_id: Option<String>,
    type_quantite: Option<String>,
}

/// Applique les champs reçus à la ligne ; le premier problème rencontré est renvoyé tel quel.
fn appliquer(l: &mut LigneBase, f: &MajLigne, taux: &[(i64, String)]) -> Result<(), String> {
    if let Some(d) = &f.description {
        if d.chars().count() > LONGUEUR_MAX_DESCRIPTION {
            return Err(format!("La description est trop longue ({LONGUEUR_MAX_DESCRIPTION} caractères au maximum)."));
        }
        l.description = d.trim_end().to_string();
    }
    if let Some(r) = &f.remarque {
        if r.chars().count() > LONGUEUR_MAX_REMARQUE {
            return Err(format!("La remarque est trop longue ({LONGUEUR_MAX_REMARQUE} caractères au maximum)."));
        }
        l.remarque = r.trim().to_string();
    }
    // Titre, sous-titre, texte et saut de page n'ont ni quantité, ni prix, ni TVA.
    if !l.est_chiffre() {
        return Ok(());
    }
    if let Some(p) = &f.prix_achat {
        l.prix_achat_unitaire_e5 = match p.trim() {
            "" => None,
            t => Some(calcul::lire_decimal_e5(t).ok_or_else(|| format!("Le prix d'achat « {t} » n'est pas un montant (exemple : 12,50)."))?),
        };
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
    if let Some(t) = &f.type_quantite {
        l.type_quantite = type_quantite_connu(t.trim()).ok_or("Ce type de quantité n'existe pas.")?.0.to_string();
    }
    if let Some(t) = &f.taux_tva_id {
        let id = t.trim().parse::<i64>().ok().filter(|id| taux.iter().any(|(i, _)| i == id)).ok_or("Ce taux de TVA n'existe pas.")?;
        l.taux_tva_id = Some(id);
    }
    Ok(())
}

async fn modifier(
    mut db: Base,
    Path((id, ligne_id)): Path<(i64, i64)>,
    // Formulaire « simple » : un champ vidé arrive comme texte vide (et non comme champ absent).
    FormSimple(f): FormSimple<MajLigne>,
) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let options = Options::charger(&mut *db, Genre::Entreprise).await?;

    let mut tx = db.begin().await?;
    let totaux_avant: (i64, i64) =
        sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let courante: Option<LigneBase> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLONNES_LIGNE}, NULL::int AS taux_centiemes
         FROM devis_lignes l WHERE l.id = $1 AND l.devis_id = $2",
    )))
    .bind(ligne_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let mut ligne = courante.ok_or_else(Erreur::introuvable)?;
    // Avec un métré, la quantité est la somme des mesures : une quantité tapée à la main n'est pas prise.
    let a_metre: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM devis_metres WHERE ligne_id = $1)").bind(ligne_id).fetch_one(&mut *tx).await?;
    let f = if a_metre { MajLigne { quantite: None, ..f } } else { f };
    let montant_avant = (ligne.quantite_e5, ligne.prix_unitaire_e5);

    let message = match appliquer(&mut ligne, &f, &options.taux_tva) {
        Ok(()) => {
            sqlx::query(
                "UPDATE devis_lignes SET description = $1, quantite_e5 = $2, unite = $3, prix_unitaire_e5 = $4,
                        taux_tva_id = $5, prix_achat_unitaire_e5 = $6, remarque = $7,
                        type_quantite = $9::text::type_quantite, updated_at = now()
                 WHERE id = $8",
            )
            .bind(&ligne.description)
            .bind(ligne.quantite_e5)
            .bind(&ligne.unite)
            .bind(ligne.prix_unitaire_e5)
            .bind(ligne.taux_tva_id)
            .bind(ligne.prix_achat_unitaire_e5)
            .bind(&ligne.remarque)
            .bind(ligne_id)
            .bind(&ligne.type_quantite)
            .execute(&mut *tx)
            .await?;
            recalculer(&mut tx, id).await?;
            String::new()
        }
        // Rien n'est enregistré : le champ garde ce que la personne a tapé, avec l'explication en haut.
        Err(m) => m,
    };
    let totaux_apres: (i64, i64) =
        sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(id).fetch_one(&mut *tx).await?;
    tx.commit().await?;

    let refuse = !message.is_empty();
    let mut enc = charger_encodage(&mut *db, id, &entete, message).await?;
    if !refuse && montant_avant != (ligne.quantite_e5, ligne.prix_unitaire_e5) {
        enc.ligne_changee = Some(ligne_id);
    }
    enc.totaux_changes = totaux_avant != totaux_apres;
    Ok(Html(Maj { enc }.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Coefficient {
    coefficient: String,
}

/// Le plus grand coefficient accepté (garde-fou contre une faute de frappe : 125 au lieu de 1,25).
const COEFFICIENT_MAX_E5: i64 = 100 * 100_000;

/// Lit un coefficient tapé (« 1,25 ») : positif, au plus 100.
fn lire_coefficient(texte: &str) -> Result<i64, String> {
    let t = texte.trim();
    match calcul::lire_decimal_e5(t) {
        Some(c) if c > 0 && c <= COEFFICIENT_MAX_E5 => Ok(c),
        _ => Err(format!("Le coefficient « {t} » n'est pas valable : un nombre plus grand que 0, au plus 100 (exemple : 1,35).")),
    }
}

/// Un coefficient tapé dans une ligne : prix de vente = prix d'achat × coefficient, arrondi au centime.
/// Le coefficient n'est pas enregistré : il se relit toujours depuis les deux prix.
async fn appliquer_coefficient(
    mut db: Base,
    Path((id, ligne_id)): Path<(i64, i64)>,
    FormSimple(f): FormSimple<Coefficient>,
) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = db.begin().await?;
    let totaux_avant: (i64, i64) =
        sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let ligne: Option<(String, Option<i64>)> = sqlx::query_as(
        "SELECT genre::text, prix_achat_unitaire_e5 FROM devis_lignes WHERE id = $1 AND devis_id = $2",
    )
    .bind(ligne_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let (genre, achat) = ligne.ok_or_else(Erreur::introuvable)?;
    let resultat = match (GENRES_CHIFFRES.contains(&genre.as_str()), achat) {
        // Champ vidé : rien à calculer, le prix reste.
        _ if f.coefficient.trim().is_empty() => Ok(None),
        (false, _) => Err("Cette ligne n'a pas de prix.".to_string()),
        (true, Some(a)) if a > 0 => lire_coefficient(&f.coefficient).map(|c| Some(calcul::prix_par_coefficient_e5(a, c))),
        (true, _) => Err("Saisissez d'abord le prix d'achat de la ligne : le coefficient le multiplie.".to_string()),
    };
    let message = match resultat {
        Ok(None) => String::new(),
        Ok(Some(prix)) => {
            sqlx::query("UPDATE devis_lignes SET prix_unitaire_e5 = $1, updated_at = now() WHERE id = $2")
                .bind(prix)
                .bind(ligne_id)
                .execute(&mut *tx)
                .await?;
            recalculer(&mut tx, id).await?;
            String::new()
        }
        Err(m) => m,
    };
    let totaux_apres: (i64, i64) =
        sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    let mut enc = charger_encodage(&mut *db, id, &entete, message).await?;
    enc.prix_change = Some(ligne_id);
    enc.totaux_changes = totaux_avant != totaux_apres;
    if enc.totaux_changes {
        enc.ligne_changee = Some(ligne_id);
    }
    Ok(Html(Maj { enc }.render()?))
}

async fn supprimer(mut db: Base, Path((id, ligne_id)): Path<(i64, i64)>) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = db.begin().await?;
    sqlx::query("SELECT id FROM devis WHERE id = $1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    sqlx::query("DELETE FROM devis_lignes WHERE id = $1 AND devis_id = $2").bind(ligne_id).bind(id).execute(&mut *tx).await?;
    recalculer(&mut tx, id).await?;
    tx.commit().await?;
    let enc = charger_encodage(&mut *db, id, &entete, String::new()).await?;
    Ok(Html(Bloc { enc }.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct InterrupteurMarge {
    marge_activee: Option<String>,
}

/// Allume ou éteint l'option « marge et prix d'achat ». Les prix d'achat déjà encodés sont gardés.
async fn basculer_marge(
    mut db: Base,
    Path(id): Path<i64>,
    FormSimple(f): FormSimple<InterrupteurMarge>,
) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    sqlx::query("UPDATE devis SET marge_activee = $1, updated_at = now() WHERE id = $2 AND statut = 'brouillon'")
        .bind(f.marge_activee.is_some())
        .bind(id)
        .execute(&mut *db)
        .await?;
    let entete = charger_entete(&mut *db, id).await?;
    let enc = charger_encodage(&mut *db, id, &entete, String::new()).await?;
    Ok(Html(Bloc { enc }.render()?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FraisGeneraux {
    frais_generaux: String,
}

/// Frais généraux : un pourcentage des achats, de 0 à 100 %, au centième près.
fn lire_frais_generaux(texte: &str) -> Result<i64, &'static str> {
    let e5 = calcul::lire_decimal_e5(texte.trim()).filter(|v| *v >= 0).ok_or("Les frais généraux sont un pourcentage (exemple : 10 ou 7,5).")?;
    if e5 % 1000 != 0 {
        return Err("Les frais généraux peuvent avoir 2 décimales au maximum.");
    }
    let centiemes = e5 / 1000;
    if centiemes > 10_000 {
        return Err("Les frais généraux ne peuvent pas dépasser 100 %.");
    }
    Ok(centiemes)
}

async fn changer_frais_generaux(
    mut db: Base,
    Path(id): Path<i64>,
    FormSimple(f): FormSimple<FraisGeneraux>,
) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    // Un champ vidé remet les frais généraux à 0.
    let texte = if f.frais_generaux.trim().is_empty() { "0" } else { f.frais_generaux.as_str() };
    let message = match lire_frais_generaux(texte) {
        Ok(centiemes) => {
            sqlx::query("UPDATE devis SET frais_generaux_centiemes = $1, updated_at = now() WHERE id = $2 AND statut = 'brouillon'")
                .bind(centiemes as i32)
                .bind(id)
                .execute(&mut *db)
                .await?;
            String::new()
        }
        Err(m) => m.to_string(),
    };
    let entete = charger_entete(&mut *db, id).await?;
    let enc = charger_encodage(&mut *db, id, &entete, message).await?;
    Ok(Html(Bloc { enc }.render()?))
}

/// Échange la ligne avec sa voisine (`vers_le_haut` : celle du dessus). Sans voisine, ne fait rien.
async fn deplacer(mut db: Base, id: i64, ligne_id: i64, vers_le_haut: bool) -> Result<Html<String>, Erreur> {
    let entete = charger_entete(&mut *db, id).await?;
    exiger_brouillon(&entete)?;
    let mut tx = db.begin().await?;
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
    let enc = charger_encodage(&mut *db, id, &entete, String::new()).await?;
    Ok(Html(Bloc { enc }.render()?))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/devis/{id}/encodage", get(page))
        .route("/devis/{id}/lignes", post(ajouter))
        .route("/devis/{id}/bibliotheque", post(ajouter_ouvrage))
        .route("/devis/{id}/marge", post(basculer_marge))
        .route("/devis/{id}/frais-generaux", post(changer_frais_generaux))
        .route("/devis/{id}/lignes/{ligne_id}", post(modifier))
        .route("/devis/{id}/lignes/{ligne_id}/supprimer", post(supprimer))
        .route("/devis/{id}/lignes/{ligne_id}/coefficient", post(appliquer_coefficient))
        .route(
            "/devis/{id}/lignes/{ligne_id}/monter",
            post(|db: Base, Path((id, l)): Path<(i64, i64)>| deplacer(db, id, l, true)),
        )
        .route(
            "/devis/{id}/lignes/{ligne_id}/descendre",
            post(|db: Base, Path((id, l)): Path<(i64, i64)>| deplacer(db, id, l, false)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frais_generaux_lus_en_pour_cent() {
        assert_eq!(lire_frais_generaux("10"), Ok(1_000));
        assert_eq!(lire_frais_generaux("7,5"), Ok(750));
        assert_eq!(lire_frais_generaux("0"), Ok(0));
        assert_eq!(lire_frais_generaux("100"), Ok(10_000));
        assert!(lire_frais_generaux("100,01").is_err());
        assert!(lire_frais_generaux("-1").is_err());
        assert!(lire_frais_generaux("abc").is_err());
        assert!(lire_frais_generaux("1,234").is_err());
        assert!(lire_frais_generaux("").is_err());
    }

    #[test]
    fn seuls_les_types_chiffres_portent_un_prix() {
        for (genre, _) in GENRES {
            assert_eq!(GENRES_CHIFFRES.contains(&genre), calcul::Categorie::depuis_genre(genre).is_some(), "{genre}");
        }
    }
}
