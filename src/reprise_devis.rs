//! Reprise des données de Vertuoza (transition, étape 2) : les devis, les avenants et les chantiers.
//!
//! Les devis sont dans le même fichier JSON que les factures (clé `devis`, voir `reprise.rs`) et passent par la même
//! commande, dans la même transaction. Un devis repris garde le numéro, la date, le statut, la décision du client et le
//! client de Vertuoza. Ses lignes sont reprises quand le fichier les donne (obligatoire pour un devis accepté : les
//! avancements de l'étape 3 portent sur les lignes) ; sans lignes, le total de Vertuoza est gardé tel quel.
//!
//! - Un devis **accepté** reçoit son chantier (même numérotation que dans l'application, dans l'ordre des acceptations).
//! - Un **avenant** est rattaché au chantier de son devis de base (`devis_base`) avec son numéro dans le chantier.
//! - Les factures reprises sont ensuite rattachées au chantier par `chantier_vertuoza` (la référence du chantier).
//! - Relancer la reprise ne crée aucun doublon ; un devis créé dans Ubifacture avec le même numéro bloque tout.

use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use sqlx::{Postgres, Transaction};

use crate::commun::format_euros;
use crate::journal::{self, Genre};
use crate::reprise::{Client, Rapport, Totaux, centimes};

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Adresse {
    #[serde(default)]
    pub rue: String,
    #[serde(default)]
    pub numero: String,
    #[serde(default)]
    pub code_postal: String,
    #[serde(default)]
    pub ville: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AjustementReprise {
    #[serde(default)]
    pub libelle: String,
    /// « pourcentage » ou « montant ».
    pub mode: String,
    /// Négatif = remise : « -5 » (5 %) ou « -100,00 » (100 € HT).
    pub valeur: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct LigneDevisReprise {
    /// titre, sous_titre, texte, poste, ouvrage, fourniture, equipement, main_oeuvre, sous_traitant, saut_de_page.
    #[serde(default = "poste")]
    pub genre: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub quantite: String,
    #[serde(default)]
    pub unite: String,
    #[serde(default)]
    pub prix_unitaire: String,
    #[serde(default)]
    pub prix_achat: String,
    /// Taux de TVA en centièmes (600 = 6 %) ; à défaut, celui du devis.
    #[serde(default)]
    pub tva: Option<i32>,
    #[serde(default)]
    pub remarque: String,
}

fn poste() -> String {
    "poste".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct DevisReprise {
    /// « devis » ou « avenant ».
    #[serde(default = "devis")]
    pub genre: String,
    /// Un devis : son numéro (2026-57 → 57). Un avenant : son numéro dans le chantier (le premier est le n° 2).
    pub numero: i64,
    /// Un avenant : le numéro du devis de base (accepté) dont il corrige le chantier.
    #[serde(default)]
    pub devis_base: Option<i64>,
    /// Un avenant : l'année du devis de base (par défaut, celle de la date de l'avenant). La numérotation est annuelle.
    #[serde(default)]
    pub annee_base: Option<i32>,
    pub date: NaiveDate,
    /// brouillon, envoye, signe, accepte, refuse.
    #[serde(default = "envoye")]
    pub statut: String,
    /// La date de l'acceptation ou du refus.
    #[serde(default)]
    pub date_decision: Option<NaiveDate>,
    #[serde(default)]
    pub motif_refus: String,
    pub client: Client,
    /// Le client à qui l'on facture, quand ce n'est pas le client du devis.
    #[serde(default)]
    pub client_facturation: Option<Client>,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub responsable: String,
    /// La référence du chantier dans Vertuoza (« PL45 »).
    #[serde(default)]
    pub chantier: String,
    #[serde(default)]
    pub conditions_jours: Option<i32>,
    /// Taux de TVA par défaut du devis, en centièmes.
    #[serde(default)]
    pub tva_defaut: Option<i32>,
    /// L'adresse du chantier, quand ce n'est pas celle du client.
    #[serde(default)]
    pub adresse_chantier: Option<Adresse>,
    #[serde(default)]
    pub remarque_1: String,
    #[serde(default)]
    pub remarque_2: String,
    #[serde(default)]
    pub jours_ouvrables: i32,
    #[serde(default)]
    pub ajustement: Option<AjustementReprise>,
    #[serde(default)]
    pub marge_activee: bool,
    /// Frais généraux en centièmes de pour cent des achats (1000 = 10 %).
    #[serde(default)]
    pub frais_generaux: i32,
    /// Un devis accepté : le début du chantier (par défaut la date de l'acceptation).
    #[serde(default)]
    pub chantier_debut: Option<NaiveDate>,
    /// Un devis accepté : le chantier est-il terminé dans Vertuoza ?
    #[serde(default)]
    pub chantier_termine: bool,
    #[serde(default)]
    pub lignes: Vec<LigneDevisReprise>,
    pub totaux: Totaux,
}

fn devis() -> String {
    "devis".to_string()
}

fn envoye() -> String {
    "envoye".to_string()
}

const STATUTS: [&str; 5] = ["brouillon", "envoye", "signe", "accepte", "refuse"];

/// Le genre de ligne de la base, et s'il porte quantité, prix et TVA.
fn genre_ligne(genre: &str) -> Option<(&'static str, bool)> {
    Some(match genre {
        "titre" | "lot" => ("titre", false),
        "sous_titre" => ("sous_titre", false),
        "texte" => ("texte", false),
        "saut_de_page" => ("saut_de_page", false),
        "poste" | "poste_libre" => ("poste_libre", true),
        "ouvrage" => ("ouvrage", true),
        "fourniture" => ("fourniture", true),
        "equipement" => ("equipement", true),
        "main_oeuvre" => ("main_oeuvre", true),
        "sous_traitant" => ("sous_traitant", true),
        _ => return None,
    })
}

fn date_belge(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

/// Un devis déjà présent dans la base : (id, repris ?, id du chantier).
type Present = (i64, bool, Option<i64>);

/// Les devis et les avenants du fichier. Renvoie le nombre de chantiers rattachés plus tard par `rattacher_factures`.
pub async fn importer(tx: &mut Transaction<'_, Postgres>, pieces: &[DevisReprise], rapport: &mut Rapport) -> Result<(), String> {
    let erreur_base = |e: sqlx::Error| format!("Erreur de la base : {e}");
    // Les devis de base d'abord, dans l'ordre des acceptations (c'est l'ordre des numéros de chantier), puis les avenants.
    let mut a_traiter: Vec<&DevisReprise> = pieces.iter().collect();
    a_traiter.sort_by_key(|p| (p.genre == "avenant", p.date_decision.unwrap_or(p.date), p.date, p.numero));

    for p in a_traiter {
        let avenant = match p.genre.as_str() {
            "devis" => false,
            "avenant" => true,
            autre => return Err(format!("Devis n° {} : type inconnu « {autre} » (devis ou avenant).", p.numero)),
        };
        let nom = if avenant {
            format!("Avenant n° {} (devis de base n° {})", p.numero, p.devis_base.map(|n| n.to_string()).unwrap_or_default())
        } else {
            format!("Devis n° {}-{}", p.date.year(), p.numero)
        };
        if !STATUTS.contains(&p.statut.as_str()) {
            return Err(format!("{nom} : statut inconnu « {} » ({}).", p.statut, STATUTS.join(", ")));
        }
        if p.numero <= 0 {
            return Err(format!("{nom} : le numéro doit être positif."));
        }

        // Le chantier de l'avenant : celui du devis de base.
        let chantier_avenant: Option<i64> = if avenant {
            let base = p.devis_base.ok_or_else(|| format!("{nom} : il manque le devis de base (devis_base)."))?;
            let annee_base = p.annee_base.unwrap_or_else(|| p.date.year());
            let (base_statut, chantier): (String, Option<i64>) = sqlx::query_as(
                "SELECT d.statut::text, ch.id FROM devis d LEFT JOIN chantiers ch ON ch.devis_id = d.id
                 WHERE d.numero = $1 AND EXTRACT(YEAR FROM d.date_devis) = $2 AND d.chantier_id IS NULL",
            )
            .bind(base)
            .bind(f64::from(annee_base))
            .fetch_optional(&mut **tx)
            .await
            .map_err(erreur_base)?
            .ok_or_else(|| format!("{nom} : le devis de base n° {annee_base}-{base} n'est pas dans la reprise."))?;
            if base_statut != "accepte" {
                return Err(format!("{nom} : le devis de base n° {base} n'est pas accepté, il n'a pas de chantier."));
            }
            Some(chantier.ok_or_else(|| format!("{nom} : le devis de base n° {base} n'a pas de chantier."))?)
        } else {
            None
        };
        if avenant && p.numero < 2 {
            return Err(format!("{nom} : le premier avenant d'un chantier est le n° 2 (le devis de base est le n° 1)."));
        }

        // Déjà là ?
        let present: Option<Present> = match chantier_avenant {
            Some(ch) => sqlx::query_as("SELECT d.id, d.reprise, d.chantier_id FROM devis d WHERE d.chantier_id = $1 AND d.numero = $2")
                .bind(ch)
                .bind(p.numero)
                .fetch_optional(&mut **tx)
                .await
                .map_err(erreur_base)?,
            None => sqlx::query_as(
                "SELECT d.id, d.reprise, ch.id FROM devis d LEFT JOIN chantiers ch ON ch.devis_id = d.id
                 WHERE d.numero = $1 AND EXTRACT(YEAR FROM d.date_devis) = $2 AND d.chantier_id IS NULL",
            )
            .bind(p.numero)
            .bind(f64::from(p.date.year()))
            .fetch_optional(&mut **tx)
            .await
            .map_err(erreur_base)?,
        };
        match present {
            Some((_, true, _)) => {
                rapport.devis_deja += 1;
                continue;
            }
            Some((_, false, _)) => return Err(format!("{nom} existe déjà dans Ubifacture (créé ici, pas repris) : rien n'est importé.")),
            None => {}
        }

        // Les totaux de Vertuoza.
        let (Some(ht), Some(tva), Some(ttc)) = (centimes(&p.totaux.ht), centimes(&p.totaux.tva), centimes(&p.totaux.ttc)) else {
            return Err(format!("{nom} : les totaux ne sont pas des montants."));
        };
        if ht + tva != ttc {
            rapport.alertes.push(format!("{nom} : HT + TVA ({}) ≠ TTC ({}).", format_euros(ht + tva), format_euros(ttc)));
        }

        // La décision du client.
        let decision = match p.statut.as_str() {
            "accepte" | "refuse" => Some(p.date_decision.unwrap_or_else(|| {
                rapport.alertes.push(format!("{nom} : pas de date de décision dans le fichier, la date du devis est utilisée."));
                p.date
            })),
            _ => None,
        };
        if let Some(d) = decision
            && d < p.date
        {
            rapport.alertes.push(format!("{nom} : décision du {} avant la date du devis ({}).", date_belge(d), date_belge(p.date)));
        }

        // Clients, conditions, TVA, adresse.
        let (entreprise, contact) = crate::reprise::client(tx, &p.client, rapport).await.map_err(erreur_base)?;
        let (fact_entreprise, fact_contact) = match &p.client_facturation {
            Some(c) => crate::reprise::client(tx, c, rapport).await.map_err(erreur_base)?,
            None => (None, None),
        };
        let conditions: Option<i64> = match p.conditions_jours {
            Some(0) => sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'comptant'").fetch_optional(&mut **tx).await.map_err(erreur_base)?,
            Some(j) => sqlx::query_scalar("SELECT id FROM conditions_paiement WHERE \"type\" = 'apres_facturation' AND jours = $1")
                .bind(j)
                .fetch_optional(&mut **tx)
                .await
                .map_err(erreur_base)?,
            None => None,
        };
        if p.conditions_jours.is_some() && conditions.is_none() {
            rapport.alertes.push(format!("{nom} : conditions de paiement de {} jour(s) inconnues, laissées vides.", p.conditions_jours.unwrap_or_default()));
        }
        // La TVA par défaut du devis : celle du fichier, sinon celle de la première ligne chiffrée.
        let tva_defaut = p
            .tva_defaut
            .or_else(|| p.lignes.iter().find(|l| genre_ligne(&l.genre).is_some_and(|(_, chiffre)| chiffre)).and_then(|l| l.tva));
        let tva_defaut_id = taux_id(tx, tva_defaut).await.map_err(erreur_base)?;
        let adresse = p.adresse_chantier.clone().unwrap_or_default();
        let (ajust_mode, ajust_valeur, ajust_libelle) = match &p.ajustement {
            Some(a) if a.mode == "pourcentage" || a.mode == "montant" => {
                let valeur = centimes(&a.valeur).ok_or_else(|| format!("{nom} : l'ajustement « {} » n'est pas un nombre.", a.valeur))?;
                (a.mode.as_str(), valeur, a.libelle.trim())
            }
            Some(a) => return Err(format!("{nom} : mode d'ajustement inconnu « {} » (pourcentage ou montant).", a.mode)),
            None => ("aucun", 0, ""),
        };
        if !(0..=10_000).contains(&p.frais_generaux) {
            return Err(format!("{nom} : frais généraux hors limites (0 à 10 000 centièmes de %)."));
        }
        if !(0..=999).contains(&p.jours_ouvrables) {
            return Err(format!("{nom} : jours ouvrables hors limites (0 à 999)."));
        }
        if !avenant && p.jours_ouvrables != 0 {
            rapport.alertes.push(format!("{nom} : des jours ouvrables sur un devis (seul un avenant en a) sont ignorés."));
        }

        // Le devis. Il naît dans son statut final (la reprise n'est pas une action de l'utilisateur) ; la date du
        // dernier changement de statut est celle de la décision, sinon celle du devis.
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO devis (numero, date_devis, reference, statut, statut_date, client_entreprise_id, client_contact_id, responsable,
                    taux_tva_id, conditions_paiement_id, facturation_identique, facturation_entreprise_id, facturation_contact_id,
                    chantier_identique, chantier_rue, chantier_numero, chantier_code_postal, chantier_ville,
                    remarque_1, remarque_2, ajustement_libelle, ajustement_mode, ajustement_valeur, marge_activee, frais_generaux_centiemes,
                    chantier_id, jours_ouvrables, date_acceptation, date_refus, motif_refus, reprise, chantier_vertuoza)
             VALUES ($1, $2, $3, $4::text::statut_devis, (COALESCE($5, $2)::timestamp + time '12:00') AT TIME ZONE 'Europe/Brussels', $6, $7, $8,
                     $9, $10, $11, $12, $13,
                     $14, $15, $16, $17, $18,
                     $19, $20, $21, $22, $23, $24, $25,
                     $26, $27, $28, $29, $30, true, $31)
             RETURNING id",
        )
        .bind(p.numero)
        .bind(p.date)
        .bind(p.reference.trim())
        .bind(&p.statut)
        .bind(decision)
        .bind(entreprise)
        .bind(contact)
        .bind(p.responsable.trim())
        .bind(tva_defaut_id)
        .bind(conditions)
        .bind(p.client_facturation.is_none())
        .bind(fact_entreprise)
        .bind(fact_contact)
        .bind(p.adresse_chantier.is_none())
        .bind(adresse.rue.trim())
        .bind(adresse.numero.trim())
        .bind(adresse.code_postal.trim())
        .bind(adresse.ville.trim())
        .bind(p.remarque_1.trim())
        .bind(p.remarque_2.trim())
        .bind(ajust_libelle)
        .bind(ajust_mode)
        .bind(ajust_valeur)
        .bind(p.marge_activee)
        .bind(p.frais_generaux)
        .bind(chantier_avenant)
        .bind(if avenant { p.jours_ouvrables } else { 0 })
        .bind(decision.filter(|_| p.statut == "accepte"))
        .bind(decision.filter(|_| p.statut == "refuse"))
        .bind(if p.statut == "refuse" { p.motif_refus.trim() } else { "" })
        .bind(p.chantier.trim())
        .fetch_one(&mut **tx)
        .await
        .map_err(erreur_base)?;

        // Les lignes.
        for (i, l) in p.lignes.iter().enumerate() {
            let (genre, chiffre) = genre_ligne(&l.genre).ok_or_else(|| format!("{nom}, ligne {} : type de ligne inconnu « {} ».", i + 1, l.genre))?;
            if !chiffre {
                sqlx::query("INSERT INTO devis_lignes (devis_id, position, genre, description, remarque) VALUES ($1, $2, $3::text::genre_ligne, $4, $5)")
                    .bind(id)
                    .bind(i as i32 + 1)
                    .bind(genre)
                    .bind(l.description.trim())
                    .bind(l.remarque.trim())
                    .execute(&mut **tx)
                    .await
                    .map_err(erreur_base)?;
                continue;
            }
            let lire = |texte: &str, champ: &str| -> Result<Option<i64>, String> {
                if texte.trim().is_empty() {
                    return Ok(None);
                }
                crate::calcul::lire_decimal_e5(texte).map(Some).ok_or_else(|| format!("{nom}, ligne {} : {champ} « {texte} » n'est pas un nombre.", i + 1))
            };
            let quantite = lire(&l.quantite, "la quantité")?.or(Some(100_000));
            let prix = lire(&l.prix_unitaire, "le prix unitaire")?;
            let achat = lire(&l.prix_achat, "le prix d'achat")?;
            let taux = l.tva.or(tva_defaut);
            let ligne_taux_id = taux_id(tx, taux).await.map_err(erreur_base)?;
            if ligne_taux_id.is_none() {
                rapport.alertes.push(format!("{nom}, ligne {} : taux de TVA {} inconnu.", i + 1, taux.map(crate::calcul::taux_en_texte).unwrap_or_else(|| "absent".into())));
            }
            if prix.is_none() {
                rapport.alertes.push(format!("{nom}, ligne {} : pas de prix unitaire.", i + 1));
            }
            sqlx::query(
                "INSERT INTO devis_lignes (devis_id, position, genre, description, quantite_e5, unite, prix_unitaire_e5, prix_achat_unitaire_e5, taux_tva_id, remarque)
                 VALUES ($1, $2, $3::text::genre_ligne, $4, $5, $6, $7, $8, $9, $10)",
            )
            .bind(id)
            .bind(i as i32 + 1)
            .bind(genre)
            .bind(l.description.trim())
            .bind(quantite)
            .bind(l.unite.trim())
            .bind(prix)
            .bind(achat)
            .bind(ligne_taux_id)
            .bind(l.remarque.trim())
            .execute(&mut **tx)
            .await
            .map_err(erreur_base)?;
        }

        // Les totaux : calculés par l'application quand il y a des lignes (ils doivent coller à ceux de Vertuoza),
        // sinon ceux de Vertuoza.
        if p.lignes.is_empty() {
            sqlx::query("UPDATE devis SET montant_ht_cents = $2, montant_ttc_cents = $3 WHERE id = $1").bind(id).bind(ht).bind(ttc).execute(&mut **tx).await.map_err(erreur_base)?;
            if p.statut == "accepte" {
                rapport.alertes.push(format!("{nom} : accepté mais sans lignes dans le fichier : pas d'avancement possible tant qu'elles ne sont pas reprises."));
            }
        } else {
            crate::devis_lignes::recalculer(tx, id).await.map_err(erreur_base)?;
            let (calcule_ht, calcule_ttc): (i64, i64) =
                sqlx::query_as("SELECT montant_ht_cents, montant_ttc_cents FROM devis WHERE id = $1").bind(id).fetch_one(&mut **tx).await.map_err(erreur_base)?;
            if (calcule_ht, calcule_ttc) != (ht, ttc) {
                rapport.alertes.push(format!(
                    "{nom} : les lignes donnent {} HT / {} TTC, Vertuoza indique {} HT / {} TTC. Les montants calculés par Ubifacture sont gardés.",
                    format_euros(calcule_ht),
                    format_euros(calcule_ttc),
                    format_euros(ht),
                    format_euros(ttc)
                ));
            }
        }

        // L'historique, puis le chantier d'un devis accepté.
        journal::noter(tx, id, Genre::Creation, &format!("Repris de Vertuoza : {nom}")).await.map_err(erreur_base)?;
        if let Some(d) = decision {
            let libelle = if p.statut == "accepte" { "Accepté" } else { "Refusé" };
            journal::noter(tx, id, Genre::Statut, &format!("{libelle} (décision du {})", date_belge(d))).await.map_err(erreur_base)?;
        }
        if p.statut == "accepte" && !avenant {
            let debut = p.chantier_debut.or(decision).unwrap_or(p.date);
            let numero = crate::chantiers::creer(tx, id, debut).await.map_err(erreur_base)?;
            journal::noter(tx, id, Genre::Chantier, &format!("Chantier n° {numero} créé")).await.map_err(erreur_base)?;
            if p.chantier_termine {
                sqlx::query("UPDATE chantiers SET statut = 'termine' WHERE devis_id = $1").bind(id).execute(&mut **tx).await.map_err(erreur_base)?;
            }
            rapport.chantiers += 1;
        } else if p.chantier_termine || p.chantier_debut.is_some() {
            rapport.alertes.push(format!("{nom} : début ou fin de chantier donnés pour un devis qui n'est pas un devis de base accepté : ignorés."));
        }

        if avenant {
            rapport.avenants += 1;
        } else {
            rapport.devis += 1;
        }
        if p.statut == "accepte" {
            rapport.accepte_ht_cents += ht;
        }
        let entree = rapport.devis_par_statut.entry(p.statut.clone()).or_insert((0, 0));
        entree.0 += 1;
        entree.1 += ht;
    }
    Ok(())
}

/// L'identifiant du taux de TVA (le premier actif à ce taux), s'il existe.
async fn taux_id(tx: &mut Transaction<'_, Postgres>, taux: Option<i32>) -> Result<Option<i64>, sqlx::Error> {
    let Some(taux) = taux else { return Ok(None) };
    sqlx::query_scalar("SELECT id FROM taux_tva WHERE taux_centiemes = $1 ORDER BY actif DESC, id LIMIT 1").bind(taux).fetch_optional(&mut **tx).await
}

/// Le compteur des devis ne redescend jamais sous le plus grand numéro de devis repris de l'année la plus récente
/// (la numérotation est annuelle, comme celle des factures).
pub async fn remonter_compteur(tx: &mut Transaction<'_, Postgres>, rapport: &mut Rapport) -> Result<(), sqlx::Error> {
    let (valeur, annee): (i64, Option<i32>) = sqlx::query_as("SELECT valeur, annee FROM compteurs WHERE organisation_id = app_organisation() AND nom = 'devis' FOR UPDATE").fetch_one(&mut **tx).await?;
    let max: Option<(i32, i64)> = sqlx::query_as(
        "SELECT EXTRACT(YEAR FROM date_devis)::int AS a, max(numero) FROM devis WHERE chantier_id IS NULL GROUP BY a ORDER BY a DESC LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((a, n)) = max {
        let (nouvelle_valeur, nouvelle_annee) = match annee {
            Some(x) if x > a => (valeur, x),
            Some(x) if x == a => (valeur.max(n), x),
            _ => (n, a),
        };
        if (nouvelle_valeur, Some(nouvelle_annee)) != (valeur, annee) {
            rapport.alertes.push(format!(
                "Compteur des devis : {valeur} ({}) → {nouvelle_valeur} ({nouvelle_annee}).",
                annee.map(|a| a.to_string()).unwrap_or_default()
            ));
            sqlx::query("UPDATE compteurs SET valeur = $1, annee = $2 WHERE organisation_id = app_organisation() AND nom = 'devis'").bind(nouvelle_valeur).bind(nouvelle_annee).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

/// Rattache les factures reprises au chantier de leur devis de base (même référence de chantier dans Vertuoza).
/// Une référence portée par plusieurs chantiers n'est pas devinée : alerte.
pub async fn rattacher_factures(tx: &mut Transaction<'_, Postgres>, rapport: &mut Rapport) -> Result<(), sqlx::Error> {
    let chantiers: Vec<(String, i64)> = sqlx::query_as(
        "SELECT lower(trim(d.chantier_vertuoza)), ch.id FROM chantiers ch JOIN devis d ON d.id = ch.devis_id
         WHERE d.reprise AND trim(d.chantier_vertuoza) <> '' ORDER BY ch.id",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut vus: std::collections::BTreeMap<String, Vec<i64>> = Default::default();
    for (reference, id) in chantiers {
        vus.entry(reference).or_default().push(id);
    }
    for (reference, ids) in &vus {
        if ids.len() > 1 {
            rapport.alertes.push(format!("Le chantier « {reference} » correspond à {} devis de base : ses factures ne sont pas rattachées.", ids.len()));
            continue;
        }
        let fait = sqlx::query("UPDATE factures SET chantier_id = $2 WHERE reprise AND chantier_id IS NULL AND lower(trim(chantier_vertuoza)) = $1")
            .bind(reference)
            .bind(ids[0])
            .execute(&mut **tx)
            .await?;
        rapport.factures_rattachees += fait.rows_affected() as usize;
    }
    let orphelines: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT trim(chantier_vertuoza) FROM factures WHERE reprise AND chantier_id IS NULL AND trim(chantier_vertuoza) <> '' ORDER BY 1",
    )
    .fetch_all(&mut **tx)
    .await?;
    if !orphelines.is_empty() {
        rapport.alertes.push(format!("Factures reprises sans chantier dans Ubifacture (référence de chantier) : {}.", orphelines.join(", ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_genres_de_ligne() {
        assert_eq!(genre_ligne("lot"), Some(("titre", false)));
        assert_eq!(genre_ligne("poste"), Some(("poste_libre", true)));
        assert_eq!(genre_ligne("main_oeuvre"), Some(("main_oeuvre", true)));
        assert_eq!(genre_ligne("inconnu"), None);
    }
}
