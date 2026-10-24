//! Les fichiers du site servis par Raven lui-même (mise en ligne, étape 7-2) : le CSS de Tailwind fabriqué depuis les
//! gabarits, celui de DaisyUI, htmx et les polices. Ils sont dans le programme (`include_bytes!`) : rien à copier à
//! côté sur le serveur, et aucune page ne dépend d'un autre site. Si un CDN tombe, Raven s'affiche quand même, et les
//! polices ne passent plus par Google (qui voyait l'adresse IP de chaque visiteur).
//!
//! L'adresse porte l'empreinte du contenu (`/ressources/raven.css?v=1a2b3c4d5e6f`, voir `adresse`) : le navigateur
//! garde le fichier un an et prend le nouveau dès qu'il change. Le CSS et le script partent compressés (gzip) quand le
//! navigateur le permet.
//!
//! `raven.css` ne contient que les classes de Tailwind employées dans `templates/` et `src/` : après un changement de
//! classes, le refabriquer avec `scripts/css.ps1` (les tests de GitHub vérifient qu'il est à jour).

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::LazyLock;

use axum::{
    Router,
    extract::{Path, Query},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::AppState;

struct Fichier {
    nom: &'static str,
    genre: &'static str,
    contenu: &'static [u8],
}

const fn entree(nom: &'static str, genre: &'static str, contenu: &'static [u8]) -> Fichier {
    Fichier { nom, genre, contenu }
}

const CSS: &str = "text/css; charset=utf-8";
const POLICE: &str = "font/ttf";

static FICHIERS: &[Fichier] = &[
    entree("raven.css", CSS, include_bytes!("../assets/web/raven.css")),
    entree("daisyui.css", CSS, include_bytes!("../assets/web/daisyui.css")),
    entree("htmx.min.js", "text/javascript; charset=utf-8", include_bytes!("../assets/web/htmx.min.js")),
    entree("Archivo-Regular.ttf", POLICE, include_bytes!("../assets/polices/Archivo-Regular.ttf")),
    entree("Archivo-Medium.ttf", POLICE, include_bytes!("../assets/polices/Archivo-Medium.ttf")),
    entree("Archivo-SemiBold.ttf", POLICE, include_bytes!("../assets/polices/Archivo-SemiBold.ttf")),
    entree("Archivo-Bold.ttf", POLICE, include_bytes!("../assets/polices/Archivo-Bold.ttf")),
    entree("IBMPlexMono-Regular.ttf", POLICE, include_bytes!("../assets/polices/IBMPlexMono-Regular.ttf")),
    entree("IBMPlexMono-Medium.ttf", POLICE, include_bytes!("../assets/polices/IBMPlexMono-Medium.ttf")),
    entree("IBMPlexMono-SemiBold.ttf", POLICE, include_bytes!("../assets/polices/IBMPlexMono-SemiBold.ttf")),
];

fn fichier(nom: &str) -> Option<&'static Fichier> {
    FICHIERS.iter().find(|f| f.nom == nom)
}

/// L'empreinte de chaque fichier : les 12 premiers caractères de son SHA-256.
static EMPREINTES: LazyLock<HashMap<&'static str, String>> =
    LazyLock::new(|| FICHIERS.iter().map(|f| (f.nom, hex::encode(Sha256::digest(f.contenu))[..12].to_string())).collect());

/// Le CSS et le script compressés une fois pour toutes, à la première demande.
static COMPRESSES: LazyLock<HashMap<&'static str, Vec<u8>>> = LazyLock::new(|| {
    FICHIERS
        .iter()
        .filter(|f| f.genre != POLICE)
        .map(|f| {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            gz.write_all(f.contenu).expect("compression en mémoire");
            (f.nom, gz.finish().expect("compression en mémoire"))
        })
        .collect()
});

/// L'adresse d'un fichier pour les gabarits, avec l'empreinte de son contenu (`/ressources/raven.css?v=…`).
pub fn adresse(nom: &str) -> String {
    match EMPREINTES.get(nom) {
        Some(empreinte) => format!("/ressources/{nom}?v={empreinte}"),
        None => panic!("fichier du site inconnu : {nom}"),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Version {
    v: String,
}

async fn servir(Path(nom): Path<String>, Query(version): Query<Version>, entetes: HeaderMap) -> Response {
    let Some(f) = fichier(&nom) else { return StatusCode::NOT_FOUND.into_response() };
    // Avec la bonne empreinte, le fichier ne changera jamais à cette adresse : gardé un an. Sinon (une page plus
    // ancienne que le fichier), le navigateur redemande à chaque fois.
    let cache = if EMPREINTES.get(f.nom) == Some(&version.v) { "public, max-age=31536000, immutable" } else { "no-cache" };
    let gzip = entetes
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|e| e.trim().starts_with("gzip")));
    let mut reponse = match COMPRESSES.get(f.nom).filter(|_| gzip) {
        Some(compresse) => {
            let mut r = compresse.as_slice().into_response();
            r.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
            r
        }
        None => f.contenu.into_response(),
    };
    let h = reponse.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(f.genre));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if f.genre != POLICE {
        h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    reponse
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/ressources/{nom}", get(servir))
}
