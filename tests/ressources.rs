// Mise en ligne (étape 7-2) : le CSS, htmx et les polices sont servis par Raven lui-même (plus aucun autre site), sans
// connexion, gardés un an par le navigateur grâce à l'empreinte de l'adresse, compressés quand c'est possible ; les pages
// n'ont le droit d'exécuter que les scripts de Raven.

mod commun;

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower::ServiceExt;
use ubifacture::{AppState, application, ressources};

async fn lire(site: &Router, chemin: &str, gzip: bool) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut r = Request::builder().uri(chemin);
    if gzip {
        r = r.header(header::ACCEPT_ENCODING, "gzip, deflate, br");
    }
    let reponse = site.clone().oneshot(r.body(Body::empty()).unwrap()).await.unwrap();
    let statut = reponse.status();
    let entetes = reponse.headers().clone();
    (statut, entetes, reponse.into_body().collect().await.unwrap().to_bytes().to_vec())
}

fn entete(h: &HeaderMap, nom: header::HeaderName) -> String {
    h.get(nom).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default()
}

#[sqlx::test]
async fn les_fichiers_du_site_sont_servis_par_raven_sans_connexion(options: PgPoolOptions, connexion: PgConnectOptions) {
    let site = application(AppState { db: commun::reserve(options, connexion).await });

    // Le site protégé : aucune connexion n'est demandée pour ces fichiers (les pages publiques en ont besoin).
    let adresse = ressources::adresse("htmx.min.js");
    assert!(adresse.starts_with("/ressources/htmx.min.js?v=") && adresse.len() == "/ressources/htmx.min.js?v=".len() + 12, "{adresse}");
    let (statut, h, corps) = lire(&site, &adresse, false).await;
    assert_eq!(statut, StatusCode::OK);
    assert_eq!(entete(&h, header::CONTENT_TYPE), "text/javascript; charset=utf-8");
    assert_eq!(entete(&h, header::CACHE_CONTROL), "public, max-age=31536000, immutable", "la bonne empreinte : gardé un an");
    assert!(String::from_utf8_lossy(&corps).starts_with("var htmx=function()"), "le fichier tel quel, non compressé sans gzip");

    // Une adresse sans empreinte (ou avec une ancienne) : le même fichier, que le navigateur redemande à chaque fois.
    let (statut, h, _) = lire(&site, "/ressources/htmx.min.js?v=ancienne", false).await;
    assert_eq!((statut, entete(&h, header::CACHE_CONTROL)), (StatusCode::OK, "no-cache".to_string()));

    // Compressé quand le navigateur l'accepte.
    let (_, h, compresse) = lire(&site, &ressources::adresse("daisyui.css"), true).await;
    assert_eq!(entete(&h, header::CONTENT_ENCODING), "gzip");
    assert_eq!(entete(&h, header::CONTENT_TYPE), "text/css; charset=utf-8");
    assert!(entete(&h, header::VARY).contains("Accept-Encoding"));
    assert!(compresse.starts_with(&[0x1f, 0x8b]) && compresse.len() < 1_000_000, "{} octets", compresse.len());

    // Le CSS de Tailwind fabriqué depuis les gabarits, et les polices.
    let (statut, h, corps) = lire(&site, &ressources::adresse("raven.css"), false).await;
    assert_eq!(statut, StatusCode::OK);
    assert!(String::from_utf8_lossy(&corps).contains("tailwindcss v3.4.17"));
    assert!(h.get(header::CONTENT_SECURITY_POLICY).is_none(), "la règle des pages ne s'applique qu'aux pages HTML");
    let (statut, h, corps) = lire(&site, &ressources::adresse("Archivo-Regular.ttf"), true).await;
    assert_eq!((statut, entete(&h, header::CONTENT_TYPE)), (StatusCode::OK, "font/ttf".to_string()));
    assert!(h.get(header::CONTENT_ENCODING).is_none() && corps.len() > 100_000, "une police part telle quelle");

    // Un nom inconnu, ou qui essaie de sortir du dossier : rien.
    for chemin in ["/ressources/inconnu.css", "/ressources/..%2F..%2F.env", "/ressources/%2E%2E%2FCargo.toml"] {
        assert_eq!(lire(&site, chemin, false).await.0, StatusCode::NOT_FOUND, "{chemin}");
    }
}

#[sqlx::test]
async fn les_pages_ne_chargent_plus_rien_d_un_autre_site(options: PgPoolOptions, connexion: PgConnectOptions) {
    let site = application(AppState { db: commun::reserve(options, connexion).await });
    // Une page publique (la page de connexion) : tout vient de Raven, Tailwind en dernier.
    let (_, h, corps) = lire(&site, "/connexion", false).await;
    let page = String::from_utf8(corps).unwrap();
    for ailleurs in ["cdn.tailwindcss.com", "jsdelivr", "unpkg", "googleapis", "gstatic", "https://"] {
        assert!(!page.contains(ailleurs), "{ailleurs} dans la page de connexion");
    }
    for fichier in ["daisyui.css", "raven.css", "Archivo-Regular.ttf", "IBMPlexMono-Regular.ttf"] {
        assert!(page.contains(&ressources::adresse(fichier)), "{fichier}");
    }
    assert!(page.find("raven.css").unwrap() > page.rfind("</style>").unwrap(), "Tailwind après les styles de Raven, comme avec le CDN");

    // Seuls les scripts de Raven s'exécutent.
    let politique = entete(&h, header::CONTENT_SECURITY_POLICY);
    for regle in ["default-src 'self'", "script-src 'self'", "font-src 'self'", "object-src 'none'", "frame-ancestors 'self'"] {
        assert!(politique.contains(regle), "{regle} : {politique}");
    }
    assert!(!politique.contains("unsafe-eval") && !politique.contains("script-src 'self' 'unsafe-inline'"), "{politique}");

    // Aucun gabarit ne charge quoi que ce soit d'un autre site (scripts, styles, polices).
    for entree in std::fs::read_dir("templates").unwrap() {
        let chemin = entree.unwrap().path();
        let texte = std::fs::read_to_string(&chemin).unwrap();
        for ailleurs in ["cdn.tailwindcss.com", "cdn.jsdelivr.net", "unpkg.com", "fonts.googleapis.com", "fonts.gstatic.com"] {
            assert!(!texte.contains(ailleurs), "{ailleurs} dans {}", chemin.display());
        }
        assert!(!texte.contains("<script>") && !texte.contains("<script type"), "script écrit dans {}", chemin.display());
    }
}
