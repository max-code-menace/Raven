// Outil de vérification à l'œil, ignoré par `cargo test` :
//   $env:RENDU_DEVIS = "7"; $env:RENDU_DOSSIER = "C:\chemin\dossier"
//   cargo test --test rendu_visuel -- --ignored
// Écrit une image PNG par page du PDF du devis demandé (dans la base du fichier .env).

#[tokio::test]
#[ignore]
async fn ecrire_les_pages_du_devis_en_png() {
    let (Ok(id), Ok(dossier)) = (std::env::var("RENDU_DEVIS"), std::env::var("RENDU_DOSSIER")) else {
        panic!("Il faut RENDU_DEVIS (numéro interne du devis) et RENDU_DOSSIER (dossier de sortie).");
    };
    dotenvy::dotenv().ok();
    let pool = ubifacture::db::connecter().await.unwrap_or_else(|e| panic!("base inaccessible : {e}"));
    let donnees = match ubifacture::devis_pdf::charger_donnees(&mut *pool.acquire().await.unwrap(), id.parse().unwrap()).await {
        Ok(d) => d,
        Err(_) => panic!("devis introuvable"),
    };
    let (texte, fichiers) = ubifacture::devis_pdf::source_typst(&donnees);
    let pages = ubifacture::pdf::pages_en_png(texte, fichiers, 1.6).unwrap();
    for (i, png) in pages.iter().enumerate() {
        std::fs::write(format!("{dossier}/page-{}.png", i + 1), png).unwrap();
    }
    println!("{} page(s) écrite(s)", pages.len());
}
