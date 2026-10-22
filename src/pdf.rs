//! Génération de PDF avec Typst (bibliothèque Rust, sans programme externe).
//! Les polices sont embarquées dans le programme : le PDF est identique sur ton PC et chez l'hébergeur.
//! Archivo et IBM Plex Mono (celles du design de Raven, licences `assets/polices/OFL-Archivo.txt` et
//! `OFL-IBMPlexMono.txt`) pour le devis ; Inter (`assets/polices/OFL.txt`) pour les pièces pas encore refaites.

use std::collections::HashMap;
use std::sync::OnceLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst_layout::PagedDocument;
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_pdf::PdfOptions;

const POLICES: [&[u8]; 12] = [
    include_bytes!("../assets/polices/Inter-Regular.otf"),
    include_bytes!("../assets/polices/Inter-SemiBold.otf"),
    include_bytes!("../assets/polices/Inter-Bold.otf"),
    include_bytes!("../assets/polices/Inter-Italic.otf"),
    include_bytes!("../assets/polices/Archivo-Regular.ttf"),
    include_bytes!("../assets/polices/Archivo-Italic.ttf"),
    include_bytes!("../assets/polices/Archivo-Medium.ttf"),
    include_bytes!("../assets/polices/Archivo-SemiBold.ttf"),
    include_bytes!("../assets/polices/Archivo-Bold.ttf"),
    include_bytes!("../assets/polices/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/polices/IBMPlexMono-Medium.ttf"),
    include_bytes!("../assets/polices/IBMPlexMono-SemiBold.ttf"),
];

/// Un document Typst en mémoire : son texte, et les fichiers (logo…) qu'il peut citer par leur nom.
struct Monde {
    partage: &'static Partage,
    id_principal: FileId,
    principal: Source,
    fichiers: HashMap<String, Bytes>,
}

/// Ce qui ne change jamais d'un devis à l'autre : la bibliothèque Typst et les polices.
/// On le prépare une seule fois, au premier PDF.
struct Partage {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    polices: Vec<Font>,
}

fn partage() -> &'static Partage {
    static PARTAGE: OnceLock<Partage> = OnceLock::new();
    PARTAGE.get_or_init(|| {
        let polices: Vec<Font> = POLICES.iter().flat_map(|p| Font::iter(Bytes::new(*p))).collect();
        let book = FontBook::from_fonts(&polices);
        Partage { library: LazyHash::new(Library::default()), book: LazyHash::new(book), polices }
    })
}

fn identifiant(chemin: &str) -> FileId {
    RootedPath::new(VirtualRoot::Project, VirtualPath::new(chemin).expect("chemin interne valide")).intern()
}

impl Monde {
    fn new(texte: String, fichiers: HashMap<String, Vec<u8>>) -> Self {
        let id_principal = identifiant("/main.typ");
        Monde {
            partage: partage(),
            id_principal,
            principal: Source::new(id_principal, texte),
            fichiers: fichiers.into_iter().map(|(nom, octets)| (format!("/{nom}"), Bytes::new(octets))).collect(),
        }
    }
}

impl World for Monde {
    fn library(&self) -> &LazyHash<Library> {
        &self.partage.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.partage.book
    }

    fn main(&self) -> FileId {
        self.id_principal
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.id_principal {
            Ok(self.principal.clone())
        } else {
            Err(FileError::NotFound(id.vpath().get_with_slash().into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let chemin = id.vpath().get_with_slash();
        self.fichiers.get(chemin).cloned().ok_or_else(|| FileError::NotFound(chemin.into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.partage.polices.get(index).cloned()
    }

    fn today(&self, _decalage: Option<Duration>) -> Option<Datetime> {
        None
    }
}

/// Compile un document Typst et renvoie le PDF. `fichiers` : images citées dans le texte par leur nom.
/// En cas d'erreur de mise en page, le message est renvoyé (il vient de notre modèle, pas de l'utilisateur).
pub fn generer(texte: String, fichiers: HashMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    let monde = Monde::new(texte, fichiers);
    let resultat = typst::compile::<PagedDocument>(&monde).output;
    let document = resultat.map_err(|erreurs| erreurs.iter().map(|e| e.message.to_string()).collect::<Vec<_>>().join(" ; "))?;
    typst_pdf::pdf(&document, &PdfOptions::default())
        .map_err(|erreurs| erreurs.iter().map(|e| e.message.to_string()).collect::<Vec<_>>().join(" ; "))
}



/// Les pages du document en images PNG, pour vérifier la mise en page à l'œil (voir `tests/rendu_visuel.rs`).
pub fn pages_en_png(texte: String, fichiers: HashMap<String, Vec<u8>>, pixels_par_pt: f64) -> Result<Vec<Vec<u8>>, String> {
    let monde = Monde::new(texte, fichiers);
    let document = typst::compile::<PagedDocument>(&monde)
        .output
        .map_err(|erreurs| erreurs.iter().map(|e| e.message.to_string()).collect::<Vec<_>>().join(" ; "))?;
    let options = typst_render::RenderOptions { pixel_per_pt: typst::utils::Scalar::new(pixels_par_pt), render_bleed: false };
    document.pages().iter().map(|page| typst_render::render(page, &options).encode_png().map_err(|e| e.to_string())).collect()
}

/// Copie dans la page les attributs qu'elle hérite normalement de ses parents (ressources, taille…),
/// pour qu'elle reste correcte une fois rattachée à un autre arbre de pages.
fn rendre_la_page_autonome(doc: &mut lopdf::Document, page: lopdf::ObjectId) {
    use lopdf::Object;
    const HERITES: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
    let mut trouves: Vec<(&[u8], Object)> = Vec::new();
    let mut courant = page;
    for _ in 0..64 {
        let Ok(dict) = doc.get_dictionary(courant) else { break };
        for cle in HERITES {
            if !trouves.iter().any(|(k, _)| *k == cle)
                && let Ok(valeur) = dict.get(cle)
            {
                trouves.push((cle, valeur.clone()));
            }
        }
        match dict.get(b"Parent").and_then(|p| p.as_reference()) {
            Ok(parent) => courant = parent,
            Err(_) => break,
        }
    }
    if let Ok(dict) = doc.get_dictionary_mut(page) {
        for (cle, valeur) in trouves {
            if !dict.has(cle) {
                dict.set(cle, valeur);
            }
        }
    }
}

/// Colle les pages de `suite` après celles de `base` (les conditions générales après le devis).
/// Renvoie une erreur si l'un des deux PDF est illisible (chiffré, abîmé) ; l'appelant garde alors `base`.
pub fn fusionner(base: &[u8], suite: &[u8]) -> Result<Vec<u8>, String> {
    use lopdf::{Document, Object, ObjectId};
    let mut documents = Vec::new();
    for (nom, octets) in [("devis", base), ("suite", suite)] {
        let doc = Document::load_mem(octets).map_err(|e| format!("PDF « {nom} » illisible : {e}"))?;
        if doc.is_encrypted() {
            return Err(format!("PDF « {nom} » chiffré"));
        }
        documents.push(doc);
    }

    let mut max_id = 1;
    let mut pages: Vec<ObjectId> = Vec::new();
    let mut objets = std::collections::BTreeMap::new();
    for doc in &mut documents {
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        let ordre: Vec<ObjectId> = doc.get_pages().into_values().collect();
        for page in &ordre {
            rendre_la_page_autonome(doc, *page);
        }
        pages.extend(ordre);
        objets.extend(std::mem::take(&mut doc.objects));
    }

    let mut resultat = Document::with_version("1.7");
    let mut catalogue: Option<(ObjectId, Object)> = None;
    let mut racine_pages: Option<ObjectId> = None;
    let mut fiches_de_pages = Vec::new();
    for (id, objet) in objets {
        match objet.type_name().unwrap_or(b"") {
            b"Catalog" => {
                if catalogue.is_none() {
                    catalogue = Some((id, objet));
                }
            }
            b"Pages" => {
                // Les anciens arbres de pages disparaissent : une seule racine, créée plus bas.
                racine_pages.get_or_insert(id);
            }
            b"Page" => fiches_de_pages.push((id, objet)),
            b"Outlines" | b"Outline" => {}
            _ => {
                resultat.objects.insert(id, objet);
            }
        }
    }
    let racine_pages = racine_pages.ok_or("arbre de pages introuvable")?;
    let (id_catalogue, catalogue) = catalogue.ok_or("catalogue introuvable")?;

    for (id, objet) in fiches_de_pages {
        if let Ok(dict) = objet.as_dict() {
            let mut dict = dict.clone();
            dict.set("Parent", racine_pages);
            resultat.objects.insert(id, Object::Dictionary(dict));
        }
    }
    let nombre = pages.len() as u32;
    let mut arbre = lopdf::Dictionary::new();
    arbre.set("Type", "Pages");
    arbre.set("Count", nombre);
    arbre.set("Kids", pages.into_iter().map(Object::Reference).collect::<Vec<_>>());
    resultat.objects.insert(racine_pages, Object::Dictionary(arbre));
    let mut dict_catalogue = catalogue.as_dict().map_err(|e| e.to_string())?.clone();
    dict_catalogue.set("Pages", racine_pages);
    dict_catalogue.remove(b"Outlines");
    resultat.objects.insert(id_catalogue, Object::Dictionary(dict_catalogue));
    resultat.trailer.set("Root", id_catalogue);
    resultat.max_id = resultat.objects.keys().map(|(n, _)| *n).max().unwrap_or(0);
    resultat.renumber_objects();
    resultat.compress();

    let mut sortie = Vec::new();
    resultat.save_to(&mut sortie).map_err(|e| e.to_string())?;
    Ok(sortie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_pdf_minimal_se_genere() {
        let pdf = generer("#set text(font: \"Inter\")\nBonjour, élève : 1 234,50 €".to_string(), HashMap::new()).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.len() > 1_000);
    }

    #[test]
    fn deux_pdf_se_collent_page_apres_page() {
        let devis = generer("A
#pagebreak()
B".to_string(), HashMap::new()).unwrap();
        let suite = generer("C
#pagebreak()
D
#pagebreak()
E".to_string(), HashMap::new()).unwrap();
        let fusion = fusionner(&devis, &suite).unwrap();
        assert!(fusion.starts_with(b"%PDF-"));
        let doc = lopdf::Document::load_mem(&fusion).unwrap();
        assert_eq!(doc.get_pages().len(), 5);
        // Chaque page a gardé ses ressources (police, taille) : elles ne viennent plus du parent.
        for (_, page) in doc.get_pages() {
            let dict = doc.get_dictionary(page).unwrap();
            assert!(dict.has(b"MediaBox") && dict.has(b"Resources"));
        }
    }

    #[test]
    fn un_pdf_illisible_n_est_pas_fusionne() {
        let bon = generer("A".to_string(), HashMap::new()).unwrap();
        assert!(fusionner(&bon, b"pas un pdf").is_err());
        assert!(fusionner(b"%PDF-1.7 abime", &bon).is_err());
    }

    #[test]
    fn une_erreur_de_modele_est_renvoyee_en_texte() {
        let erreur = generer("#inconnu()".to_string(), HashMap::new()).unwrap_err();
        assert!(!erreur.is_empty());
    }
}
