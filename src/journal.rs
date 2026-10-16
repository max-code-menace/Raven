//! Le journal d'un devis : une ligne par événement important (création, changement de statut,
//! téléchargement du PDF, duplication, création du chantier). On écrit dans la même transaction que le changement lui-même :
//! pas d'événement sans changement, pas de changement sans événement.

use sqlx::{Postgres, Transaction};

/// Les genres d'événements connus de la base (voir la contrainte de `devis_evenements`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Genre {
    Creation,
    Statut,
    Pdf,
    Duplication,
    Chantier,
    Email,
    Client,
}

impl Genre {
    pub fn code(self) -> &'static str {
        match self {
            Genre::Creation => "creation",
            Genre::Statut => "statut",
            Genre::Pdf => "pdf",
            Genre::Duplication => "duplication",
            Genre::Chantier => "chantier",
            Genre::Email => "email",
            Genre::Client => "client",
        }
    }

    /// Le libellé affiché dans l'historique.
    pub fn libelle(code: &str) -> &'static str {
        match code {
            "creation" => "Création",
            "statut" => "Statut",
            "pdf" => "PDF",
            "duplication" => "Duplication",
            "chantier" => "Chantier",
            "email" => "E-mail",
            "client" => "Client",
            _ => "Événement",
        }
    }
}

pub async fn noter(tx: &mut Transaction<'_, Postgres>, devis_id: i64, genre: Genre, detail: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO devis_evenements (devis_id, genre, detail) VALUES ($1, $2, $3)")
        .bind(devis_id)
        .bind(genre.code())
        .bind(detail)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_genres_ont_un_code_et_un_libelle() {
        for g in [Genre::Creation, Genre::Statut, Genre::Pdf, Genre::Duplication, Genre::Chantier, Genre::Email, Genre::Client] {
            assert_ne!(Genre::libelle(g.code()), "Événement", "{g:?}");
        }
        assert_eq!(Genre::libelle("inconnu"), "Événement");
    }
}
