//! Spelling dictionaries from LibreOffice's dictionary collection
//! (<https://github.com/LibreOffice/dictionaries>), downloaded on request into WordCraft's
//! dictionaries folder ([`crate::hunspell::download_dir`]): the one the user chose in File ›
//! Options › Proofing, else WordCraft's own.
//!
//! WordCraft doesn't bundle these files: most are GPL, LGPL or MPL, and WordCraft is MIT OR
//! Apache-2.0. Downloaded at the user's request, a dictionary stays a separate data file under
//! its own licence that WordCraft reads at runtime, like one installed by the system or by
//! LibreOffice. This module only knows where each language's pair lives and checks and saves
//! what was downloaded; the download itself belongs to the front end (the web has no folder).
//!
//! The collection has a folder for some 70 languages (`it_IT`, `pl_PL`, `pt_BR`…), each with a
//! `<name>.aff`/`<name>.dic` pair and a README giving its licence. [`source`] maps the languages
//! WordCraft proofs today; a language that gets a [`ProofLang`] only needs a row here.

use std::path::{Path, PathBuf};

use crate::ProofLang;

/// The collection's raw-file root.
pub const BASE_URL: &str = "https://raw.githubusercontent.com/LibreOffice/dictionaries/master";

/// Largest `.aff` or `.dic` file accepted: what [`crate::hunspell::load`] reads (Turkish's
/// `.dic`, the largest, is ~36 MB).
pub const MAX_FILE: usize = crate::hunspell::MAX_FILE as usize;

/// Where a language's dictionary lives in the collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    /// The folder in the collection (`it_IT`).
    pub folder: &'static str,
    /// The file name without extension (`it_IT`): the pair is `<name>.aff` and `<name>.dic`,
    /// saved under the same names so [`crate::hunspell::find`] picks them up.
    pub name: &'static str,
}

impl Source {
    /// The `.aff` URL.
    pub fn aff_url(&self) -> String {
        format!("{BASE_URL}/{}/{}.aff", self.folder, self.name)
    }

    /// The `.dic` URL.
    pub fn dic_url(&self) -> String {
        format!("{BASE_URL}/{}/{}.dic", self.folder, self.name)
    }

    /// The folder's page, with the README that gives the dictionary's authors and licence.
    pub fn page_url(&self) -> String {
        format!("https://github.com/LibreOffice/dictionaries/tree/master/{}", self.folder)
    }
}

/// Where `lang`'s dictionary is in the collection; `None` for English (bundled), for languages
/// WordCraft can't proof yet, and for Croatian and Turkish, whose collection dictionaries use
/// two-character and numeric affix flags (`FLAG long`, `FLAG num`) that [`crate::affix`] doesn't
/// read yet: [`install`] would refuse them.
pub fn source(lang: ProofLang) -> Option<Source> {
    let (folder, name) = match lang {
        ProofLang::Pl => ("pl_PL", "pl_PL"),
        ProofLang::Pt => ("pt_BR", "pt_BR"),
        ProofLang::It => ("it_IT", "it_IT"),
        ProofLang::Ru => ("ru_RU", "ru_RU"),
        ProofLang::Cs => ("cs_CZ", "cs_CZ"),
        ProofLang::En | ProofLang::Hr | ProofLang::Tr | ProofLang::Bg | ProofLang::Other => return None,
    };
    Some(Source { folder, name })
}

/// The languages a dictionary can be downloaded for, in display order.
pub fn languages() -> impl Iterator<Item = ProofLang> {
    ProofLang::PROOFED.into_iter().filter(|l| source(*l).is_some())
}

/// Check a downloaded pair and save it in `dir` as `<name>.aff`/`<name>.dic`, replacing an
/// earlier download. The pair must decode and parse as a non-empty Hunspell dictionary; each
/// file is written next to its final name and renamed into place, so a failed or partial
/// download never leaves a broken pair behind. Returns the `.dic` path.
pub fn install(dir: &Path, source: &Source, aff: &[u8], dic: &[u8]) -> Result<PathBuf, String> {
    for (bytes, what) in [(aff, ".aff"), (dic, ".dic")] {
        if bytes.is_empty() {
            return Err(format!("the downloaded {what} file is empty"));
        }
        if bytes.len() > MAX_FILE {
            return Err(format!("the downloaded {what} file is too large ({} bytes)", bytes.len()));
        }
    }
    let (aff_text, dic_text) = crate::hunspell::decode(aff, dic)?;
    let parsed = crate::affix::AffixDict::from_hunspell(&aff_text, &dic_text, &[])?;
    if parsed.is_empty() {
        return Err("the downloaded dictionary has no words".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let aff_path = dir.join(format!("{}.aff", source.name));
    let dic_path = dir.join(format!("{}.dic", source.name));
    for (bytes, path) in [(aff, &aff_path), (dic, &dic_path)] {
        let mut part = path.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        std::fs::write(&part, bytes).map_err(|e| format!("{}: {e}", part.display()))?;
        if let Err(e) = std::fs::rename(&part, path) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{}: {e}", path.display()));
        }
    }
    Ok(dic_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_languages_whose_collection_dictionary_loads_have_a_source() {
        for lang in ProofLang::PROOFED {
            let expected = !matches!(lang, ProofLang::En | ProofLang::Hr | ProofLang::Tr);
            assert_eq!(source(lang).is_some(), expected, "{lang:?}");
        }
        assert_eq!(source(ProofLang::Other), None);
        let it = source(ProofLang::It).unwrap();
        assert_eq!(it.aff_url(), "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/it_IT/it_IT.aff");
        assert_eq!(it.dic_url(), "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/it_IT/it_IT.dic");
        let listed: Vec<ProofLang> = crate::installed_dictionaries().into_iter().map(|(l, _)| l).collect();
        assert!(languages().all(|l| listed.contains(&l)), "every downloadable language is in File › Options › Proofing");
        assert_eq!(languages().collect::<Vec<_>>(), [ProofLang::Pl, ProofLang::Pt, ProofLang::It, ProofLang::Ru, ProofLang::Cs]);
    }

    #[test]
    fn install_saves_a_valid_pair_and_refuses_a_broken_one() {
        let dir = std::env::temp_dir().join(format!("wordcraft-lo-dict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = source(ProofLang::It).unwrap();
        let aff = b"SET UTF-8\nSFX A Y 1\nSFX A o i o\n";
        assert!(install(&dir, &src, aff, b"").is_err(), "empty .dic");
        assert!(install(&dir, &src, aff, b"0\n").is_err(), "no words");
        assert!(install(&dir, &src, b"SET EBCDIC\n", b"1\ngatto\n").is_err(), "unknown encoding");
        assert!(!dir.join("it_IT.dic").exists(), "nothing saved after a refused download");
        let dic = install(&dir, &src, aff, b"2\ngatto/A\ncane\n").unwrap();
        assert_eq!(dic, dir.join("it_IT.dic"));
        assert_eq!(crate::hunspell::find_in(&dir, &["it_IT"]), Some((dir.join("it_IT.aff"), dic.clone())));
        let d = crate::hunspell::load(&dir.join("it_IT.aff"), &dic).unwrap();
        assert!(d.check("gatti") && d.check("cane") && !d.check("gatte"));
        assert!(!dir.join("it_IT.aff.part").exists() && !dir.join("it_IT.dic.part").exists(), "no leftovers");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
