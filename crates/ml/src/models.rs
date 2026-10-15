//! Finding model files: a speech model folder, and the translation models in a folder of them.

use std::path::{Path, PathBuf};

use crate::{Error, Result, lang};

/// Most folders looked at when scanning for translation models.
const MAX_SCANNED: usize = 2000;

/// The files a Whisper model folder needs.
pub const SPEECH_FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];
/// The files a Marian translation model folder needs (the weights may also be `pytorch_model.bin`).
pub const TRANSLATION_FILES: [&str; 4] = ["config.json", "source.spm", "target.spm", "vocab.json"];

/// Check that `dir` holds a speech model, saying which file is missing if not.
pub fn check_speech_model(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Err(Error::Model(format!("no speech model folder at {}", dir.display())));
    }
    for f in SPEECH_FILES {
        if !dir.join(f).is_file() {
            return Err(Error::Model(format!("the speech model folder {} has no {f}", dir.display())));
        }
    }
    Ok(())
}

/// The weights file of a translation model folder: `model.safetensors`, else `pytorch_model.bin`.
pub fn translation_weights(dir: &Path) -> Option<PathBuf> {
    ["model.safetensors", "pytorch_model.bin"].iter().map(|f| dir.join(f)).find(|p| p.is_file())
}

/// A translation model on disk and the language pair it translates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationModel {
    pub dir: PathBuf,
    /// Source language code (two letters where one exists), or a group such as `mul`.
    pub from: String,
    /// Target language code, or a group such as `mul` (such models pick the language from a
    /// `>>xx<<` token).
    pub to: String,
}

impl TranslationModel {
    /// Whether this model translates `from` (any source when `None`) into `to`.
    pub fn covers(&self, from: Option<&str>, to: &str) -> bool {
        let to = lang::normalize(to);
        let from_ok = from.is_none_or(|f| lang::normalize(f) == self.from || is_group(&self.from));
        from_ok && (self.to == to || is_group(&self.to))
    }

    /// Whether the model translates into several languages and needs to be told which.
    pub fn multi_target(&self) -> bool {
        is_group(&self.to)
    }
}

/// A code that names a group of languages (`mul`, `ROMANCE`, `gem`…) rather than one: anything
/// that isn't two letters after normalising.
fn is_group(code: &str) -> bool {
    code.len() != 2
}

/// The language pair a model folder's name gives: `opus-mt-en-he`, `opus-mt-tc-big-he-en`,
/// `en-he`… The last two `-` separated parts are the source and target.
pub fn pair_from_name(name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = name.split(['-', '_']).filter(|p| !p.is_empty()).collect();
    let (to, from) = (parts.last()?, parts.get(parts.len().checked_sub(2)?)?);
    let ok = |p: &str| (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphabetic());
    (ok(from) && ok(to)).then(|| {
        let norm = |p: &str| if p.chars().all(|c| c.is_ascii_uppercase()) && p.len() > 3 { p.to_string() } else { lang::normalize(p) };
        (norm(from), norm(to))
    })
}

/// Whether `dir` holds a translation model's files.
pub fn is_translation_model(dir: &Path) -> bool {
    TRANSLATION_FILES.iter().all(|f| dir.join(f).is_file()) && translation_weights(dir).is_some()
}

/// The pair a model in `dir` translates: from its folder name, else from the name recorded in
/// its `config.json` (`_name_or_path`).
fn pair_of(dir: &Path) -> Option<(String, String)> {
    if let Some(p) = dir.file_name().and_then(|n| n.to_str()).and_then(pair_from_name) {
        return Some(p);
    }
    let cfg = crate::read_text_file(&dir.join("config.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&cfg).ok()?;
    let name = v.get("_name_or_path")?.as_str()?;
    pair_from_name(name.rsplit(['/', '\\']).next()?)
}

/// The translation models in `folder`: the folder itself if it is one, and its subfolders that
/// are (one level down), sorted by pair.
pub fn scan_translation_models(folder: &Path) -> Vec<TranslationModel> {
    let mut out = Vec::new();
    let mut dirs = vec![folder.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(folder) {
        dirs.extend(rd.flatten().take(MAX_SCANNED).map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for dir in dirs {
        if is_translation_model(&dir)
            && let Some((from, to)) = pair_of(&dir)
        {
            out.push(TranslationModel { dir, from, to });
        }
    }
    out.sort_by(|a, b| (&a.from, &a.to, &a.dir).cmp(&(&b.from, &b.to, &b.dir)));
    out
}

/// The best model in `models` for `from` → `to`: an exact pair first, then one into several
/// languages or from several.
pub fn pick<'a>(models: &'a [TranslationModel], from: Option<&str>, to: &str) -> Option<&'a TranslationModel> {
    let to_n = lang::normalize(to);
    let from_n = from.map(lang::normalize);
    let exact = |m: &&TranslationModel| m.to == to_n && from_n.as_ref().is_none_or(|f| *f == m.from);
    models.iter().find(exact).or_else(|| models.iter().find(|m| m.covers(from, to)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_come_from_folder_names() {
        assert_eq!(pair_from_name("opus-mt-en-he"), Some(("en".into(), "he".into())));
        assert_eq!(pair_from_name("opus-mt-tc-big-he-en"), Some(("he".into(), "en".into())));
        assert_eq!(pair_from_name("opus-mt-en-ROMANCE"), Some(("en".into(), "ROMANCE".into())));
        assert_eq!(pair_from_name("opus-mt_tiny_eng-rus"), Some(("en".into(), "ru".into())));
        assert_eq!(pair_from_name("model"), None);
        assert_eq!(pair_from_name("en-123"), None);
        assert_eq!(pair_from_name(""), None);
    }

    #[test]
    fn picking_prefers_the_exact_pair() {
        let m = |f: &str, t: &str| TranslationModel { dir: PathBuf::from(format!("{f}-{t}")), from: f.into(), to: t.into() };
        let models = vec![m("en", "mul"), m("en", "he"), m("he", "en"), m("mul", "en")];
        assert_eq!(pick(&models, Some("en"), "he").unwrap().dir, PathBuf::from("en-he"));
        assert_eq!(pick(&models, Some("en-US"), "fr").unwrap().dir, PathBuf::from("en-mul"));
        assert_eq!(pick(&models, Some("de"), "en").unwrap().dir, PathBuf::from("mul-en"));
        assert_eq!(pick(&models, None, "en").unwrap().dir, PathBuf::from("he-en"));
        assert_eq!(pick(&models, Some("he"), "fr"), None);
        assert!(m("en", "mul").multi_target() && !m("en", "he").multi_target());
    }

    #[test]
    fn scanning_finds_complete_model_folders() {
        let base = std::env::temp_dir().join(format!("wc-ml-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let full = base.join("opus-mt-en-he");
        let partial = base.join("opus-mt-he-en");
        for d in [&full, &partial] {
            std::fs::create_dir_all(d).unwrap();
        }
        for f in TRANSLATION_FILES.iter().chain(&["model.safetensors"]) {
            std::fs::write(full.join(f), "x").unwrap();
        }
        std::fs::write(partial.join("config.json"), "{}").unwrap();
        let found = scan_translation_models(&base);
        assert_eq!(found, vec![TranslationModel { dir: full.clone(), from: "en".into(), to: "he".into() }]);
        // The model folder itself works too.
        assert_eq!(scan_translation_models(&full).len(), 1);
        assert!(scan_translation_models(&base.join("missing")).is_empty());
        assert!(check_speech_model(&full).is_err());
        assert!(check_speech_model(&base.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
