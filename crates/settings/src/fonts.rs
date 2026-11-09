//! The fonts installed on this machine, for choosing one by name.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledFont {
    pub name: String,
    pub monospace: bool,
}

/// One entry per family, sorted by name. This reads every font file, which takes a moment,
/// so call it off the UI thread.
pub fn installed_fonts() -> Vec<InstalledFont> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    tidy(database.faces().filter_map(|face| {
        let (name, _) = face.families.first()?;
        Some(InstalledFont {
            name: name.clone(),
            monospace: face.monospaced,
        })
    }))
}

/// A family counts as monospaced if any of its faces is. Names starting with a dot are
/// macOS's private fonts, which can't be asked for by name.
fn tidy(faces: impl Iterator<Item = InstalledFont>) -> Vec<InstalledFont> {
    let mut fonts: Vec<InstalledFont> = Vec::new();
    for face in faces.filter(|face| !face.name.is_empty() && !face.name.starts_with('.')) {
        match fonts.iter_mut().find(|font| font.name == face.name) {
            Some(font) => font.monospace |= face.monospace,
            None => fonts.push(face),
        }
    }
    fonts.sort_by_key(|font| font.name.to_lowercase());
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(name: &str, monospace: bool) -> InstalledFont {
        InstalledFont {
            name: name.to_owned(),
            monospace,
        }
    }

    #[test]
    fn lists_each_family_once_in_order_without_private_ones() {
        let tidied = tidy(
            [
                font("Menlo", false),
                font(".SF NS", false),
                font("arial", false),
                font("Menlo", true),
                font("", false),
                font("Zapfino", false),
            ]
            .into_iter(),
        );
        assert_eq!(
            tidied,
            [
                font("arial", false),
                font("Menlo", true),
                font("Zapfino", false)
            ]
        );
    }
}
