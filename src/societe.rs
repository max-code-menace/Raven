//! Paramètres de la société : identité, adresse, coordonnées, TVA, comptes bancaires,
//! logo et conditions générales (PDF joint aux devis et aux factures).

use askama::Template;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Multipart, Query},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::{Connection as _, FromRow, PgConnection};

use crate::base::Base;
use crate::commun::{Choix, PAYS, choix_fixes, email_valide, vide_en_none};
use crate::{AppState, Erreur};

const TAILLE_MAX_LOGO: usize = 1024 * 1024;
const TAILLE_MAX_PDF: usize = 2 * 1024 * 1024;
/// Limite de la requête entière : un peu plus que le plus gros fichier, pour afficher
/// notre propre message plutôt qu'une page d'erreur brute.
const LIMITE_REQUETE: usize = 3 * 1024 * 1024;

// ─── Vérifications ──────────────────────────────────────────────────────────

/// Numéro de TVA : retire les espaces et points, vérifie la clé des numéros belges.
/// Renvoie le numéro mis en forme (« BE 0726 407 561 » pour la Belgique).
pub fn normaliser_tva(saisie: &str, pays: &str) -> Result<String, &'static str> {
    let mut t: String = saisie.chars().filter(|c| !matches!(c, ' ' | '.' | '-' | '\u{a0}')).collect::<String>().to_uppercase();
    if t.is_empty() {
        return Err("Le numéro de TVA est vide.");
    }
    // Un numéro belge tapé sans « BE » est complété.
    if pays == "BE" && t.chars().all(|c| c.is_ascii_digit()) && (t.len() == 9 || t.len() == 10) {
        if t.len() == 9 {
            t.insert(0, '0');
        }
        t.insert_str(0, "BE");
    }
    if let Some(chiffres) = t.strip_prefix("BE") {
        let valide = chiffres.len() == 10
            && chiffres.chars().all(|c| c.is_ascii_digit())
            && (chiffres.starts_with('0') || chiffres.starts_with('1'))
            && {
                let base: u64 = chiffres[..8].parse().unwrap_or(0);
                let cle: u64 = chiffres[8..].parse().unwrap_or(0);
                97 - (base % 97) == cle
            };
        if !valide {
            return Err("Ce numéro de TVA belge n'est pas valide (exemple : BE 0726 407 561).");
        }
        return Ok(format!("BE {} {} {}", &chiffres[..4], &chiffres[4..7], &chiffres[7..]));
    }
    let lettres_ok = t.chars().take(2).all(|c| c.is_ascii_uppercase());
    if t.len() < 4 || t.len() > 14 || !lettres_ok || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Ce numéro de TVA n'est pas valide (il commence par le code du pays, exemple : FR12345678901).");
    }
    Ok(t)
}

/// IBAN : retire les espaces, vérifie la clé (modulo 97). Renvoie l'IBAN groupé par 4.
pub fn normaliser_iban(saisie: &str) -> Result<String, &'static str> {
    let t: String = saisie.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    let forme_ok = (15..=34).contains(&t.len())
        && t.chars().all(|c| c.is_ascii_alphanumeric())
        && t.chars().take(2).all(|c| c.is_ascii_uppercase())
        && t.chars().skip(2).take(2).all(|c| c.is_ascii_digit());
    if !forme_ok {
        return Err("Cet IBAN n'est pas valide.");
    }
    let mut reste: u32 = 0;
    for c in t[4..].chars().chain(t[..4].chars()) {
        let valeur = c.to_digit(36).expect("caractère alphanumérique");
        reste = if valeur >= 10 { (reste * 100 + valeur) % 97 } else { (reste * 10 + valeur) % 97 };
    }
    if reste != 1 {
        return Err("Cet IBAN n'est pas valide (la clé de contrôle ne correspond pas).");
    }
    let groupes: Vec<String> = t.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
    Ok(groupes.join(" "))
}

pub fn bic_valide(bic: &str) -> bool {
    matches!(bic.len(), 8 | 11) && bic.chars().all(|c| c.is_ascii_alphanumeric()) && bic.chars().take(6).all(|c| c.is_ascii_uppercase())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fichier {
    Logo,
    ConditionsGenerales,
}

impl Fichier {
    fn usage(self) -> &'static str {
        match self {
            Fichier::Logo => "logo",
            Fichier::ConditionsGenerales => "conditions_generales",
        }
    }
    fn taille_max(self) -> usize {
        match self {
            Fichier::Logo => TAILLE_MAX_LOGO,
            Fichier::ConditionsGenerales => TAILLE_MAX_PDF,
        }
    }
    fn nom_lisible(self) -> &'static str {
        match self {
            Fichier::Logo => "Le logo",
            Fichier::ConditionsGenerales => "Le fichier des conditions générales",
        }
    }
}

/// Reconnaît le vrai type du fichier d'après son contenu, pas d'après son nom ni ce que
/// déclare le navigateur. Les images SVG sont refusées : elles peuvent contenir du code.
fn type_du_fichier(fichier: Fichier, contenu: &[u8]) -> Option<&'static str> {
    match fichier {
        Fichier::Logo if contenu.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) => Some("image/png"),
        Fichier::Logo if contenu.starts_with(&[0xFF, 0xD8, 0xFF]) => Some("image/jpeg"),
        Fichier::ConditionsGenerales if contenu.starts_with(b"%PDF-") => Some("application/pdf"),
        _ => None,
    }
}

/// Nom de fichier sûr : lettres, chiffres, point, tiret, souligné, espace.
fn nom_de_fichier_propre(brut: &str, defaut: &str) -> String {
    let dernier = brut.rsplit(['/', '\\']).next().unwrap_or("");
    let propre: String = dernier
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' })
        .collect();
    let propre = propre.trim().trim_start_matches('.').to_string();
    if propre.is_empty() { defaut.to_string() } else { propre.chars().take(100).collect() }
}

// ─── Formulaire ─────────────────────────────────────────────────────────────

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct SocieteForm {
    nom: String,
    nom_abrege: String,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
    email: String,
    emails_supplementaires: Vec<String>,
    telephone: String,
    telephones_supplementaires: Vec<String>,
    site_web: String,
    numero_tva: String,
    /// « assujetti » ou « franchise » (petite entreprise).
    regime_tva: String,
    iban: Vec<String>,
    bic: Vec<String>,
}

/// La mention que la loi demande sur les documents d'une société en franchise de TVA (à faire valider par le comptable).
pub const MENTION_FRANCHISE: &str = "Régime particulier de franchise des petites entreprises.";

/// La société de l'organisation de la connexion est-elle en franchise de TVA ?
pub(crate) async fn en_franchise(db: &mut PgConnection) -> Result<bool, sqlx::Error> {
    let franchise: Option<bool> =
        sqlx::query_scalar("SELECT regime_tva = 'franchise' FROM societe WHERE organisation_id = app_organisation()").fetch_optional(db).await?;
    Ok(franchise.unwrap_or(false))
}

/// Les régimes de TVA proposés.
pub const REGIMES_TVA: [(&str, &str); 2] = [
    ("assujetti", "Assujetti à la TVA"),
    ("franchise", "Franchise de TVA des petites entreprises (pas de TVA sur les factures)"),
];

impl SocieteForm {
    /// Le formulaire rempli par l'assistant de démarrage (un téléphone, un compte, nom abrégé = nom).
    pub(crate) fn depuis_demarrage(
        nom: &str,
        rue: &str,
        numero: &str,
        code_postal: &str,
        ville: &str,
        pays: &str,
        email: &str,
        telephone: &str,
        numero_tva: &str,
        regime_tva: &str,
        iban: &str,
        bic: &str,
    ) -> SocieteForm {
        SocieteForm {
            nom: nom.to_string(),
            nom_abrege: nom.to_string(),
            rue: rue.to_string(),
            numero: numero.to_string(),
            code_postal: code_postal.to_string(),
            ville: ville.to_string(),
            pays: pays.to_string(),
            email: email.to_string(),
            telephone: telephone.to_string(),
            numero_tva: numero_tva.to_string(),
            regime_tva: regime_tva.to_string(),
            iban: vec![iban.to_string()],
            bic: vec![bic.to_string()],
            ..Default::default()
        }
    }
}

#[derive(FromRow)]
struct SocieteLigne {
    nom: String,
    nom_abrege: String,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
    email: String,
    emails_supplementaires: Vec<String>,
    telephone: String,
    telephones_supplementaires: Vec<String>,
    site_web: Option<String>,
    numero_tva: Option<String>,
    regime_tva: String,
}

#[derive(FromRow)]
struct CompteLigne {
    iban: String,
    bic: Option<String>,
}

async fn charger_formulaire(db: &mut PgConnection) -> Result<SocieteForm, sqlx::Error> {
    let societe: Option<SocieteLigne> = sqlx::query_as(
        "SELECT nom, nom_abrege, rue, numero, code_postal, ville, pays, email, emails_supplementaires,
                telephone, telephones_supplementaires, site_web, numero_tva, regime_tva FROM societe WHERE organisation_id = app_organisation()",
    )
    .fetch_optional(&mut *db)
    .await?;
    let comptes: Vec<CompteLigne> =
        sqlx::query_as("SELECT iban, bic FROM societe_comptes_bancaires WHERE organisation_id = app_organisation() ORDER BY par_defaut DESC, id")
            .fetch_all(&mut *db)
            .await?;

    let mut form = match societe {
        Some(s) => SocieteForm {
            nom: s.nom,
            nom_abrege: s.nom_abrege,
            rue: s.rue,
            numero: s.numero,
            code_postal: s.code_postal,
            ville: s.ville,
            pays: s.pays,
            email: s.email,
            emails_supplementaires: s.emails_supplementaires,
            telephone: s.telephone,
            telephones_supplementaires: s.telephones_supplementaires,
            site_web: s.site_web.unwrap_or_default(),
            numero_tva: s.numero_tva.unwrap_or_default(),
            regime_tva: s.regime_tva,
            ..Default::default()
        },
        None => SocieteForm { pays: "BE".into(), regime_tva: "assujetti".into(), ..Default::default() },
    };
    form.iban = comptes.iter().map(|c| c.iban.clone()).collect();
    form.bic = comptes.iter().map(|c| c.bic.clone().unwrap_or_default()).collect();
    Ok(form)
}

pub(crate) struct Valeurs {
    nom: String,
    nom_abrege: String,
    rue: String,
    numero: String,
    code_postal: String,
    ville: String,
    pays: String,
    email: String,
    emails_supplementaires: Vec<String>,
    telephone: String,
    telephones_supplementaires: Vec<String>,
    site_web: Option<String>,
    numero_tva: Option<String>,
    regime_tva: String,
    comptes: Vec<(String, Option<String>)>,
}

pub(crate) fn valider(f: &SocieteForm) -> Result<Valeurs, Vec<String>> {
    let mut erreurs = Vec::new();

    let nom = f.nom.trim().to_string();
    if nom.is_empty() {
        erreurs.push("Le nom de la société est obligatoire.".to_string());
    }
    let nom_abrege = f.nom_abrege.trim().to_string();
    if nom_abrege.is_empty() {
        erreurs.push("Le nom abrégé est obligatoire.".to_string());
    }

    let pays = if f.pays.is_empty() { "BE".to_string() } else { f.pays.clone() };
    if !PAYS.iter().any(|(v, _)| *v == pays) {
        erreurs.push("Ce pays n'existe pas.".to_string());
    }

    let email = f.email.trim().to_string();
    if email.is_empty() {
        erreurs.push("L'adresse e-mail est obligatoire.".to_string());
    } else if !email_valide(&email) {
        erreurs.push(format!("L'e-mail « {email} » n'est pas valide."));
    }
    let emails_supplementaires: Vec<String> =
        f.emails_supplementaires.iter().filter_map(|e| vide_en_none(e)).collect();
    for e in &emails_supplementaires {
        if !email_valide(e) {
            erreurs.push(format!("L'e-mail « {e} » n'est pas valide."));
        }
    }

    let telephone = f.telephone.trim().to_string();
    if telephone.is_empty() {
        erreurs.push("Le numéro de téléphone est obligatoire.".to_string());
    }
    let telephones_supplementaires: Vec<String> =
        f.telephones_supplementaires.iter().filter_map(|t| vide_en_none(t)).collect();

    let numero_tva = match vide_en_none(&f.numero_tva) {
        None => None,
        Some(t) => match normaliser_tva(&t, &pays) {
            Ok(n) => Some(n),
            Err(message) => {
                erreurs.push(message.to_string());
                None
            }
        },
    };

    let regime_tva = if f.regime_tva.is_empty() { "assujetti".to_string() } else { f.regime_tva.clone() };
    if !REGIMES_TVA.iter().any(|(v, _)| *v == regime_tva) {
        erreurs.push("Ce régime de TVA n'existe pas.".to_string());
    }

    // Les lignes de compte sont envoyées par paires (IBAN, BIC), dans l'ordre. Les lignes
    // entièrement vides sont ignorées. Le premier compte est celui utilisé par défaut.
    let mut comptes = Vec::new();
    for (i, iban) in f.iban.iter().enumerate() {
        let bic = f.bic.get(i).map(|b| b.trim().to_uppercase()).unwrap_or_default();
        if iban.trim().is_empty() {
            if !bic.is_empty() {
                erreurs.push("Un BIC est renseigné sans IBAN.".to_string());
            }
            continue;
        }
        match normaliser_iban(iban) {
            Ok(iban) => {
                if !bic.is_empty() && !bic_valide(&bic) {
                    erreurs.push(format!("Le BIC « {bic} » n'est pas valide."));
                }
                comptes.push((iban, (!bic.is_empty()).then_some(bic)));
            }
            Err(message) => erreurs.push(format!("{message} (« {} »)", iban.trim())),
        }
    }
    let mut vus = std::collections::HashSet::new();
    if comptes.iter().any(|(iban, _)| !vus.insert(iban.clone())) {
        erreurs.push("Le même IBAN est saisi deux fois.".to_string());
    }

    if !erreurs.is_empty() {
        return Err(erreurs);
    }
    Ok(Valeurs {
        nom,
        nom_abrege,
        rue: f.rue.trim().to_string(),
        numero: f.numero.trim().to_string(),
        code_postal: f.code_postal.trim().to_string(),
        ville: f.ville.trim().to_string(),
        pays,
        email,
        emails_supplementaires,
        telephone,
        telephones_supplementaires,
        site_web: vide_en_none(&f.site_web),
        numero_tva,
        regime_tva,
        comptes,
    })
}

pub(crate) async fn enregistrer(db: &mut PgConnection, v: &Valeurs) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO societe (nom, nom_abrege, rue, numero, code_postal, ville, pays, email,
                              emails_supplementaires, telephone, telephones_supplementaires, site_web, numero_tva, regime_tva)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT (organisation_id) DO UPDATE SET
            nom = $1, nom_abrege = $2, rue = $3, numero = $4, code_postal = $5, ville = $6, pays = $7,
            email = $8, emails_supplementaires = $9, telephone = $10, telephones_supplementaires = $11,
            site_web = $12, numero_tva = $13, regime_tva = $14, updated_at = now()",
    )
    .bind(&v.nom)
    .bind(&v.nom_abrege)
    .bind(&v.rue)
    .bind(&v.numero)
    .bind(&v.code_postal)
    .bind(&v.ville)
    .bind(&v.pays)
    .bind(&v.email)
    .bind(&v.emails_supplementaires)
    .bind(&v.telephone)
    .bind(&v.telephones_supplementaires)
    .bind(&v.site_web)
    .bind(&v.numero_tva)
    .bind(&v.regime_tva)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM societe_comptes_bancaires WHERE organisation_id = app_organisation()").execute(&mut *tx).await?;
    for (i, (iban, bic)) in v.comptes.iter().enumerate() {
        sqlx::query("INSERT INTO societe_comptes_bancaires (iban, bic, par_defaut) VALUES ($1, $2, $3)")
            .bind(iban)
            .bind(bic)
            .bind(i == 0)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

// ─── Fichiers en base ───────────────────────────────────────────────────────

async fn nom_du_fichier(db: &mut PgConnection, fichier: Fichier) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT nom FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = $1")
        .bind(fichier.usage())
        .fetch_optional(&mut *db)
        .await
}

async fn version_du_logo(db: &mut PgConnection) -> Result<i64, sqlx::Error> {
    let v: Option<i64> =
        sqlx::query_scalar("SELECT extract(epoch FROM updated_at)::bigint FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = 'logo'")
            .fetch_optional(&mut *db)
            .await?;
    Ok(v.unwrap_or(0))
}

// ─── Page ───────────────────────────────────────────────────────────────────

pub struct Compte {
    pub iban: String,
    pub bic: String,
}

#[derive(Template)]
#[template(path = "societe.html")]
struct PageSociete {
    form: SocieteForm,
    erreurs: Vec<String>,
    enregistre: bool,
    pays: Vec<Choix>,
    comptes: Vec<Compte>,
    a_un_logo: bool,
    version_logo: i64,
    conditions_generales: Option<String>,
    regimes: &'static [(&'static str, &'static str)],
}

/// Un champ ajouté par le bouton « Ajouter un e-mail » / « Ajouter un téléphone ».
#[derive(Template)]
#[template(path = "_societe_ligne_champ.html")]
struct LigneChamp {
    nom: &'static str,
    type_champ: &'static str,
    valeur: String,
}

#[derive(Template)]
#[template(path = "_societe_ligne_compte.html")]
struct LigneCompte {
    compte: Compte,
}

async fn construire_page(
    db: &mut PgConnection,
    form: SocieteForm,
    erreurs: Vec<String>,
    enregistre: bool,
) -> Result<PageSociete, sqlx::Error> {
    let mut comptes: Vec<Compte> = form
        .iban
        .iter()
        .enumerate()
        .map(|(i, iban)| Compte { iban: iban.clone(), bic: form.bic.get(i).cloned().unwrap_or_default() })
        .collect();
    if comptes.is_empty() {
        comptes.push(Compte { iban: String::new(), bic: String::new() });
    }
    let pays_actuel = if form.pays.is_empty() { "BE".to_string() } else { form.pays.clone() };
    Ok(PageSociete {
        pays: choix_fixes(&PAYS, &[pays_actuel]),
        comptes,
        a_un_logo: nom_du_fichier(&mut *db, Fichier::Logo).await?.is_some(),
        version_logo: version_du_logo(&mut *db).await?,
        conditions_generales: nom_du_fichier(&mut *db, Fichier::ConditionsGenerales).await?,
        regimes: &REGIMES_TVA,
        form,
        erreurs,
        enregistre,
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Confirmation {
    ok: String,
}

async fn afficher(mut db: Base, Query(q): Query<Confirmation>) -> Result<Html<String>, Erreur> {
    let form = charger_formulaire(&mut *db).await?;
    let page = construire_page(&mut *db, form, Vec::new(), q.ok == "1").await?;
    Ok(Html(page.render()?))
}

async fn sauvegarder(mut db: Base, Form(form): Form<SocieteForm>) -> Result<Response, Erreur> {
    match valider(&form) {
        Ok(valeurs) => {
            enregistrer(&mut *db, &valeurs).await?;
            Ok(Redirect::to("/parametres/societe?ok=1").into_response())
        }
        Err(erreurs) => {
            let page = construire_page(&mut *db, form, erreurs, false).await?;
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response())
        }
    }
}

async fn erreur_de_fichier(db: &mut PgConnection, message: String) -> Result<Response, Erreur> {
    let form = charger_formulaire(&mut *db).await?;
    let page = construire_page(&mut *db, form, vec![message], false).await?;
    Ok((StatusCode::UNPROCESSABLE_ENTITY, Html(page.render()?)).into_response())
}

async fn televerser(fichier: Fichier, mut db: Base, mut multipart: Multipart) -> Result<Response, Erreur> {
    let mut recu: Option<(String, Vec<u8>)> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(champ)) => {
                if champ.name() == Some("fichier") {
                    let nom = champ.file_name().unwrap_or("").to_string();
                    match champ.bytes().await {
                        Ok(octets) => recu = Some((nom, octets.to_vec())),
                        Err(_) => {
                            let message = format!("{} est trop volumineux ou illisible.", fichier.nom_lisible());
                            return erreur_de_fichier(&mut *db, message).await;
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => {
                let message = format!("{} est trop volumineux ou illisible.", fichier.nom_lisible());
                return erreur_de_fichier(&mut *db, message).await;
            }
        }
    }

    let Some((nom_brut, contenu)) = recu.filter(|(_, c)| !c.is_empty()) else {
        return erreur_de_fichier(&mut *db, "Choisissez un fichier avant de cliquer sur Envoyer.".to_string()).await;
    };
    if contenu.len() > fichier.taille_max() {
        let message = format!("{} dépasse {} Mo.", fichier.nom_lisible(), fichier.taille_max() / (1024 * 1024));
        return erreur_de_fichier(&mut *db, message).await;
    }
    let Some(type_mime) = type_du_fichier(fichier, &contenu) else {
        let attendu = match fichier {
            Fichier::Logo => "une image PNG ou JPEG",
            Fichier::ConditionsGenerales => "un fichier PDF",
        };
        return erreur_de_fichier(&mut *db, format!("{} doit être {attendu}.", fichier.nom_lisible())).await;
    };

    let defaut = match fichier {
        Fichier::Logo => "logo",
        Fichier::ConditionsGenerales => "conditions-generales.pdf",
    };
    let nom = nom_de_fichier_propre(&nom_brut, defaut);
    sqlx::query(
        "INSERT INTO societe_fichiers (usage, nom, type_mime, contenu) VALUES ($1, $2, $3, $4)
         ON CONFLICT (organisation_id, usage) DO UPDATE SET nom = $2, type_mime = $3, contenu = $4, updated_at = now()",
    )
    .bind(fichier.usage())
    .bind(nom)
    .bind(type_mime)
    .bind(contenu)
    .execute(&mut *db)
    .await?;
    Ok(Redirect::to("/parametres/societe?ok=1").into_response())
}

async fn retirer(fichier: Fichier, mut db: Base) -> Result<Redirect, Erreur> {
    sqlx::query("DELETE FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = $1")
        .bind(fichier.usage())
        .execute(&mut *db)
        .await?;
    Ok(Redirect::to("/parametres/societe?ok=1"))
}

async fn telecharger(fichier: Fichier, mut db: Base) -> Result<Response, Erreur> {
    let ligne: Option<(String, String, Vec<u8>)> =
        sqlx::query_as("SELECT nom, type_mime, contenu FROM societe_fichiers WHERE organisation_id = app_organisation() AND usage = $1")
            .bind(fichier.usage())
            .fetch_optional(&mut *db)
            .await?;
    let (nom, type_mime, contenu) = ligne.ok_or_else(Erreur::introuvable)?;
    Ok((
        [
            (header::CONTENT_TYPE, type_mime),
            (header::CONTENT_DISPOSITION, format!("inline; filename=\"{nom}\"")),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        contenu,
    )
        .into_response())
}

pub fn routes() -> Router<AppState> {
    let televersement = |fichier: Fichier| {
        post(move |db: Base, m: Multipart| televerser(fichier, db, m))
            .layer(DefaultBodyLimit::max(LIMITE_REQUETE))
    };
    Router::new()
        .route("/parametres/societe", get(afficher).post(sauvegarder))
        .route(
            "/parametres/societe/ligne/email",
            get(|| async {
                LigneChamp { nom: "emails_supplementaires", type_champ: "email", valeur: String::new() }
                    .render()
                    .map(Html)
                    .map_err(Erreur::from)
            }),
        )
        .route(
            "/parametres/societe/ligne/telephone",
            get(|| async {
                LigneChamp { nom: "telephones_supplementaires", type_champ: "tel", valeur: String::new() }
                    .render()
                    .map(Html)
                    .map_err(Erreur::from)
            }),
        )
        .route(
            "/parametres/societe/ligne/compte",
            get(|| async {
                LigneCompte { compte: Compte { iban: String::new(), bic: String::new() } }
                    .render()
                    .map(Html)
                    .map_err(Erreur::from)
            }),
        )
        // Réponse vide : sert au bouton « Retirer » d'une ligne ajoutée.
        .route("/parametres/societe/ligne/vide", get(|| async { Html("") }))
        .route("/parametres/societe/logo", get(|db: Base| telecharger(Fichier::Logo, db)))
        .route("/parametres/societe/logo/envoyer", televersement(Fichier::Logo))
        .route(
            "/parametres/societe/logo/retirer",
            post(|db: Base| retirer(Fichier::Logo, db)),
        )
        .route(
            "/parametres/societe/conditions-generales",
            get(|db: Base| telecharger(Fichier::ConditionsGenerales, db)),
        )
        .route("/parametres/societe/conditions-generales/envoyer", televersement(Fichier::ConditionsGenerales))
        .route(
            "/parametres/societe/conditions-generales/retirer",
            post(|db: Base| retirer(Fichier::ConditionsGenerales, db)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tva_belge_valide_ou_non() {
        assert_eq!(normaliser_tva("BE0726407561", "BE").unwrap(), "BE 0726 407 561");
        assert_eq!(normaliser_tva("be 0726.407.561", "BE").unwrap(), "BE 0726 407 561");
        assert_eq!(normaliser_tva("0726407561", "BE").unwrap(), "BE 0726 407 561");
        assert_eq!(normaliser_tva("726407561", "BE").unwrap(), "BE 0726 407 561");
        assert!(normaliser_tva("BE0726407562", "BE").is_err()); // mauvaise clé
        assert!(normaliser_tva("BE072640756", "BE").is_err()); // trop court
        assert!(normaliser_tva("BE2726407561", "BE").is_err()); // ne commence pas par 0 ou 1
        assert!(normaliser_tva("", "BE").is_err());
    }

    #[test]
    fn tva_etrangere_acceptee_si_la_forme_est_correcte() {
        assert_eq!(normaliser_tva("FR 12 345678901", "FR").unwrap(), "FR12345678901");
        assert!(normaliser_tva("12345678901", "FR").is_err());
        assert!(normaliser_tva("FR1", "FR").is_err());
    }

    #[test]
    fn iban_valide_ou_non() {
        assert_eq!(normaliser_iban("be68539007547034").unwrap(), "BE68 5390 0754 7034");
        assert_eq!(normaliser_iban("BE68 5390 0754 7034").unwrap(), "BE68 5390 0754 7034");
        assert!(normaliser_iban("BE68 5390 0754 7035").is_err());
        assert!(normaliser_iban("1234").is_err());
        assert!(normaliser_iban("").is_err());
        assert!(normaliser_iban("FR1420041010050500013M02606").is_ok());
    }

    #[test]
    fn bic() {
        assert!(bic_valide("GEBABEBB"));
        assert!(bic_valide("GEBABEBB36A"));
        assert!(!bic_valide("GEBA"));
        assert!(!bic_valide("gebabebb"));
    }

    #[test]
    fn types_de_fichiers_reconnus_par_leur_contenu() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0];
        assert_eq!(type_du_fichier(Fichier::Logo, &png), Some("image/png"));
        assert_eq!(type_du_fichier(Fichier::Logo, &[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(type_du_fichier(Fichier::Logo, b"<svg onload=alert(1)>"), None);
        assert_eq!(type_du_fichier(Fichier::Logo, b"%PDF-1.4"), None);
        assert_eq!(type_du_fichier(Fichier::ConditionsGenerales, b"%PDF-1.4"), Some("application/pdf"));
        assert_eq!(type_du_fichier(Fichier::ConditionsGenerales, &png), None);
    }

    #[test]
    fn noms_de_fichiers_assainis() {
        assert_eq!(nom_de_fichier_propre("C:\\docs\\Mes CG.pdf", "x.pdf"), "Mes CG.pdf");
        assert_eq!(nom_de_fichier_propre("../../etc/passwd", "x.pdf"), "passwd");
        assert_eq!(nom_de_fichier_propre("a\"b;c.pdf", "x.pdf"), "a_b_c.pdf");
        assert_eq!(nom_de_fichier_propre("", "x.pdf"), "x.pdf");
        assert_eq!(nom_de_fichier_propre("...", "x.pdf"), "x.pdf");
    }
}
