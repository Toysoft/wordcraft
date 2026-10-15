//! Dictate (Home › Voice) and Translate (Review › Language), with local models (`wordcraft-ml`):
//! nothing leaves the computer.
//!
//! - `tools.dictate` inserts dictated text at the caret. The app records the microphone and
//!   transcribes on a worker thread, then hands each phrase over as `text`; headless callers give
//!   a WAV file (`path`), which is transcribed here.
//! - `review.translate` translates the document (or the selected paragraphs) keeping its
//!   formatting (see [`crate::translate`]). The app translates on a worker thread and hands the
//!   results over as `translations`; headless callers let the command load the model.
//!
//! The models are the user's own files: File › Options › Language sets the speech model folder
//! and the folder of translation models (`file.options`: `speechModel`, `translationModels`), and
//! `model` overrides either for one call. `docs/local-models.md` says where to get them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};
use wordcraft_doc::StoryRef;

use crate::translate::{ApplyOptions, Plan, Scope};
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

/// Largest WAV file `tools.dictate` reads (about 90 minutes of 16-bit 48 kHz stereo).
#[cfg(not(target_arch = "wasm32"))]
const MAX_WAV_BYTES: u64 = 1 << 30;
/// Longest dictated text inserted in one call, in characters.
const MAX_DICTATED_CHARS: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("tools.dictate", "Dictate", "Home › Voice", dictate).params(
            r#"{"text"?: string (dictated text to insert at the caret), "path"?: string (a WAV file to transcribe and insert), "language"?: string (what is spoken: a code such as he or en; auto: detect, the default or File › Options' choice), "model"?: string (a Whisper model folder; default: File › Options › Language)} — one of text or path; the app's Dictate button records the microphone instead"#,
        ),
        CommandSpec::new("review.translate", "Translate", "Review › Language", translate).params(
            r#"{"to": string (target language code: he, en, de…), "from"?: string (source language; default: guessed from the text), "scope"?: "document|selection" (default document; selection: the selected paragraphs, in place), "showIn"?: "new|original" (document: a new untitled document, the default, or this one), "model"?: string (a translation model folder, or a folder of them; default: File › Options › Language), "translations"?: {text: translation} (use these instead of a model; texts missing from it stay as they are)} → counts of paragraphs and texts translated"#,
        ),
    ]
}

/// The language tag translated or dictated text is marked with: a common region for codes that
/// have one (`he` → `he-IL`), else the code itself.
pub fn lang_tag(code: &str) -> String {
    let c = wordcraft_ml::lang::normalize(code);
    let region = match c.as_str() {
        "ar" => "SA",
        "bg" => "BG",
        "cs" => "CZ",
        "da" => "DK",
        "de" => "DE",
        "el" => "GR",
        "en" => "US",
        "es" => "ES",
        "et" => "EE",
        "fa" => "IR",
        "fi" => "FI",
        "fr" => "FR",
        "he" => "IL",
        "hi" => "IN",
        "hr" => "HR",
        "hu" => "HU",
        "it" => "IT",
        "ja" => "JP",
        "ko" => "KR",
        "nl" => "NL",
        "pl" => "PL",
        "pt" => "BR",
        "ro" => "RO",
        "ru" => "RU",
        "sk" => "SK",
        "sv" => "SE",
        "th" => "TH",
        "tr" => "TR",
        "uk" => "UA",
        "vi" => "VN",
        "zh" => "CN",
        _ => "",
    };
    if region.is_empty() { c } else { format!("{c}-{region}") }
}

/// A language code from a parameter: 2–8 ASCII letters (with an optional region), normalised;
/// `auto` or empty is `None`.
fn lang_param(v: &Value, k: &str) -> Result<Option<String>, CmdError> {
    let Some(raw) = v.get(k).filter(|x| !x.is_null()) else { return Ok(None) };
    let s = raw.as_str().ok_or_else(|| CmdError::Params(format!("`{k}` must be a language code such as en or he")))?.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let primary = s.split(['-', '_']).next().unwrap_or("");
    if !(2..=8).contains(&primary.len()) || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || s.len() > 35 {
        return Err(CmdError::Params(format!("`{k}`: {s:?} isn't a language code such as en or he")));
    }
    Ok(Some(wordcraft_ml::lang::normalize(s)))
}

/// A folder given by a parameter or a setting, held to the automation read root.
fn folder(path: &str) -> Result<PathBuf, CmdError> {
    crate::file_access::check_read(path).map_err(CmdError::Failed)?;
    let p = PathBuf::from(path);
    if !p.is_dir() {
        return Err(CmdError::Failed(format!("no folder at {path}")));
    }
    Ok(p)
}

/// The text to insert for a dictated phrase after `before` (the character before the caret):
/// a space between it and the previous word, none at the start of a paragraph or after a space.
pub fn dictated_text(before: Option<char>, text: &str) -> String {
    let t: String = text.trim().chars().filter(|c| !c.is_control() || *c == '\n').take(MAX_DICTATED_CHARS).collect();
    if t.is_empty() {
        return t;
    }
    let first = t.chars().next().unwrap_or(' ');
    let joins = matches!(first, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '%' | '…' | '،' | '。' | '、');
    let needs_space = before.is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | '[' | '"' | '“' | '\u{FFFC}')) && !joins;
    if needs_space { format!(" {t}") } else { t }
}

fn dictate(s: &mut Session, v: &Value) -> CmdResult {
    let language = lang_param(v, "language")?;
    let (text, heard) = if let Some(t) = v.get("text").filter(|x| !x.is_null()) {
        (t.as_str().ok_or_else(|| CmdError::Params("`text` must be a string".into()))?.to_string(), language.clone())
    } else if let Some(path) = p::str(v, "path") {
        crate::file_access::check_read(path).map_err(CmdError::Failed)?;
        let model_dir = match p::str(v, "model") {
            Some(m) => folder(m)?,
            None => match s.prefs.speech_model.clone() {
                Some(m) => folder(&m)?,
                None => return Err(CmdError::Failed(NO_SPEECH_MODEL.into())),
            },
        };
        let language = language.clone().or_else(|| s.prefs.dictation_language.clone());
        let t = transcribe_file(path, &model_dir, language.as_deref())?;
        (t.text, t.language)
    } else {
        return Err(CmdError::Params("`text` or `path` (a WAV file) is required; the app's Dictate button records the microphone".into()));
    };
    let before = if s.sel.is_collapsed() {
        let at = &s.sel.focus;
        s.doc.para_at(at).and_then(|p| p.text.get(..at.off.min(p.text.len())).and_then(|t| t.chars().next_back()))
    } else {
        None
    };
    let insert = dictated_text(before, &text);
    if !insert.is_empty() {
        super::type_text(s, &insert)?;
    }
    Ok(json!({"text": insert.trim_start(), "language": heard}))
}

/// What to say when no speech model is set.
pub const NO_SPEECH_MODEL: &str =
    "No speech model is set. Choose a Whisper model folder in File › Options › Language (see docs/local-models.md for where to get one).";

/// Transcribe a WAV file with the model in `model_dir`.
#[cfg(not(target_arch = "wasm32"))]
fn transcribe_file(path: &str, model_dir: &Path, language: Option<&str>) -> Result<wordcraft_ml::Transcript, CmdError> {
    let bytes = crate::io::read_file_capped(path, MAX_WAV_BYTES, "recording").map_err(CmdError::Failed)?;
    let pcm = wordcraft_ml::audio::read_wav(&bytes).map_err(|e| CmdError::Failed(e.to_string()))?;
    let mut model = wordcraft_ml::SpeechModel::load(model_dir).map_err(|e| CmdError::Failed(e.to_string()))?;
    model.transcribe(&pcm.to_whisper(), language, &AtomicBool::new(false)).map_err(|e| CmdError::Failed(e.to_string()))
}

/// The web build has no models.
#[cfg(target_arch = "wasm32")]
fn transcribe_file(_path: &str, _model_dir: &Path, _language: Option<&str>) -> Result<wordcraft_ml::Transcript, CmdError> {
    Err(CmdError::Failed(wordcraft_ml::Error::Unavailable.to_string()))
}

/// The paragraphs `review.translate` works on.
pub fn translate_scope(s: &Session, selection: bool) -> Scope {
    if !selection {
        return Scope::Document;
    }
    let (a, b) = s.sel.ordered();
    let story: StoryRef = a.story;
    let paths = if a.path == b.path { vec![a.path.clone()] } else { s.doc.para_paths_in(&a, &b) };
    Scope::Paragraphs(paths.into_iter().map(|p| (story, p)).collect())
}

/// The language a plan's text is in, guessed: from its script, else the document's language.
pub fn guess_source(s: &Session, plan: &Plan) -> Option<String> {
    wordcraft_ml::lang::guess_from_script(&plan.sample(4000))
        .map(str::to_string)
        .or_else(|| super::doc_lang(&s.doc).map(|l| wordcraft_ml::lang::normalize(&l)))
        .or_else(|| Some("en".to_string()))
}

/// The translation model for `from` → `to`: `model` (a model folder or a folder of them), else
/// the folder set in File › Options.
pub fn find_translation_model(s: &Session, model: Option<&str>, from: Option<&str>, to: &str) -> Result<PathBuf, CmdError> {
    let dir = match model {
        Some(m) => folder(m)?,
        None => match s.prefs.translation_models.clone() {
            Some(m) => folder(&m)?,
            None => return Err(CmdError::Failed(no_translation_model(from, to))),
        },
    };
    let found = wordcraft_ml::models::scan_translation_models(&dir);
    wordcraft_ml::models::pick(&found, from, to).map(|m| m.dir.clone()).ok_or_else(|| CmdError::Failed(no_translation_model(from, to)))
}

/// What to say when there is no model for a pair.
pub fn no_translation_model(from: Option<&str>, to: &str) -> String {
    let to = wordcraft_ml::lang::name(to);
    match from {
        Some(f) => format!(
            "No translation model for {} → {to}. Put one (such as Helsinki-NLP's opus-mt models) in the folder set in File › Options › Language; see docs/local-models.md.",
            wordcraft_ml::lang::name(f)
        ),
        None => format!(
            "No translation model into {to}. Put one (such as Helsinki-NLP's opus-mt models) in the folder set in File › Options › Language; see docs/local-models.md."
        ),
    }
}

/// Most entries a `translations` map may have.
const MAX_TRANSLATIONS: usize = crate::translate::MAX_TEXTS;

fn translate(s: &mut Session, v: &Value) -> CmdResult {
    let to = lang_param(v, "to")?.ok_or_else(|| CmdError::Params("`to` (the language to translate into, such as he or en) is required".into()))?;
    let from = lang_param(v, "from")?;
    let selection = match p::str(v, "scope") {
        None | Some("document") => false,
        Some("selection") => true,
        Some(other) => return Err(CmdError::Params(format!("`scope` is document or selection, not {other:?}"))),
    };
    let new_document = match p::str(v, "showIn") {
        None => !selection,
        Some("new") if selection => return Err(CmdError::Params("a selection is translated in place (`showIn`: original)".into())),
        Some("new") => true,
        Some("original") => false,
        Some(other) => return Err(CmdError::Params(format!("`showIn` is new or original, not {other:?}"))),
    };
    // A model folder given is held to the automation read root even when nothing needs it.
    if let Some(m) = p::str(v, "model") {
        folder(m)?;
    }
    let plan = Plan::new(&s.doc, &translate_scope(s, selection));
    let given = match v.get("translations").filter(|x| !x.is_null()) {
        None => None,
        Some(Value::Object(m)) => {
            if m.len() > MAX_TRANSLATIONS {
                return Err(CmdError::Params(format!("`translations` takes at most {MAX_TRANSLATIONS} texts")));
            }
            let mut out = HashMap::with_capacity(m.len());
            for (k, x) in m {
                let t = x.as_str().ok_or_else(|| CmdError::Params("`translations` maps texts to translated texts (strings)".into()))?;
                out.insert(k.clone(), t.to_string());
            }
            Some(out)
        }
        Some(_) => return Err(CmdError::Params("`translations` must be an object of text → translation".into())),
    };
    let from = from.or_else(|| guess_source(s, &plan));
    if given.is_none() && from.as_deref() == Some(to.as_str()) {
        return Err(CmdError::Failed(format!("The text is already in {}.", wordcraft_ml::lang::name(&to))));
    }
    let texts = plan.texts();
    let (translations, model) = match given {
        Some(m) => (m, None),
        None if plan.is_empty() => (HashMap::new(), None),
        None => {
            let dir = find_translation_model(s, p::str(v, "model"), from.as_deref(), &to)?;
            let mut t = wordcraft_ml::Translator::load(&dir, Some(&to)).map_err(|e| CmdError::Failed(e.to_string()))?;
            let cancel = AtomicBool::new(false);
            let mut out = HashMap::with_capacity(texts.len());
            for text in &texts {
                let r = t.translate(text, &cancel).map_err(|e| CmdError::Failed(e.to_string()))?;
                out.insert(text.clone(), r);
            }
            (out, Some(dir))
        }
    };
    let done = plan.apply(&mut s.doc, &translations, &ApplyOptions { to: to.clone() });
    if new_document {
        // A new, untitled document: saving it never overwrites the original's file.
        s.path = None;
        s.file_name = None;
        s.sel = Selection::caret(s.doc.start_of(StoryRef::Body));
    }
    s.relayout();
    Ok(json!({
        "to": to,
        "from": from,
        "paragraphs": done.paragraphs,
        "translated": done.translated,
        "untranslated": done.untranslated,
        "texts": texts.len(),
        "newDocument": new_document,
        "model": model.map(|m| m.to_string_lossy().into_owned()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use wordcraft_doc::Document;

    #[test]
    fn dictated_phrases_are_spaced_like_typing() {
        assert_eq!(dictated_text(None, " Hello there. "), "Hello there.");
        assert_eq!(dictated_text(Some('.'), "Next one"), " Next one");
        assert_eq!(dictated_text(Some(' '), "word"), "word");
        assert_eq!(dictated_text(Some('d'), ", and more"), ", and more");
        assert_eq!(dictated_text(Some('('), "inside"), "inside");
        assert_eq!(dictated_text(Some('x'), "   "), "");
        assert_eq!(dictated_text(Some('ם'), "שלום"), " שלום");
        assert_eq!(dictated_text(None, "a\u{0007}b"), "ab");
    }

    #[test]
    fn dictate_inserts_text_at_the_caret_as_one_undo_step() {
        let mut s = Session::new(Document::from_text("Start."));
        s.run("caret.docEnd", &json!({})).unwrap();
        let r = s.run("tools.dictate", &json!({"text": "Then more words."})).unwrap();
        assert_eq!(r["text"], "Then more words.");
        assert_eq!(s.doc.body[0].as_para().unwrap().text, "Start. Then more words.");
        assert_eq!(s.undo_label(), Some("Dictate"));
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(s.doc.body[0].as_para().unwrap().text, "Start.");
    }

    #[test]
    fn dictate_without_text_or_audio_and_without_a_model_says_what_is_missing() {
        let mut s = Session::new(Document::new());
        let e = s.run("tools.dictate", &json!({})).unwrap_err();
        assert!(e.to_string().contains("`text` or `path`"), "{e}");
        let wav = std::env::temp_dir().join(format!("wc-dictate-{}.wav", std::process::id()));
        std::fs::write(&wav, wordcraft_ml::audio::write_wav(&[0.0; 1600], 16_000)).unwrap();
        let e = s.run("tools.dictate", &json!({"path": wav.to_string_lossy()})).unwrap_err();
        assert!(e.to_string().contains("No speech model"), "{e}");
        let e = s.run("tools.dictate", &json!({"path": wav.to_string_lossy(), "model": std::env::temp_dir().to_string_lossy()})).unwrap_err();
        assert!(e.to_string().contains("config.json"), "{e}");
        assert!(s.run("tools.dictate", &json!({"text": "x", "language": "not a language!"})).is_err());
        assert!(s.run("tools.dictate", &json!({"text": 5})).is_err());
        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn translate_with_given_translations_makes_a_new_document_and_undoes() {
        let mut s = Session::new(Document::from_text("Good morning.\nSee you."));
        s.path = Some(PathBuf::from("original.docx"));
        let r = s.run("review.translate", &json!({"to": "de", "translations": {"Good morning.": "Guten Morgen.", "See you.": "Bis bald."}})).unwrap();
        assert_eq!((r["paragraphs"].as_u64(), r["texts"].as_u64(), r["newDocument"].as_bool()), (Some(2), Some(2), Some(true)));
        assert_eq!(s.doc.body[0].as_para().unwrap().text, "Guten Morgen.");
        assert_eq!(s.doc.body[1].as_para().unwrap().text, "Bis bald.");
        assert!(s.path.is_none(), "the result is a new, untitled document");
        assert_eq!(s.undo_label(), Some("Translate"));
        s.run("edit.undo", &json!({})).unwrap();
        assert_eq!(s.doc.body[0].as_para().unwrap().text, "Good morning.");
    }

    #[test]
    fn translate_a_selection_in_place() {
        let mut s = Session::new(Document::from_text("One.\nTwo.\nThree."));
        s.path = Some(PathBuf::from("keep.docx"));
        s.sel = Selection { anchor: wordcraft_doc::Pos::body(1, 0), focus: wordcraft_doc::Pos::body(1, 2) };
        let r = s.run("review.translate", &json!({"to": "fr", "scope": "selection", "translations": {"Two.": "Deux."}})).unwrap();
        assert_eq!(r["newDocument"], false);
        let texts: Vec<String> = s.doc.body.iter().map(|b| b.as_para().unwrap().text.clone()).collect();
        assert_eq!(texts, vec!["One.", "Deux.", "Three."]);
        assert!(s.path.is_some());
        assert!(s.run("review.translate", &json!({"to": "fr", "scope": "selection", "showIn": "new", "translations": {}})).is_err());
    }

    #[test]
    fn translate_refuses_bad_parameters_and_says_when_no_model_is_set() {
        let mut s = Session::new(Document::from_text("Hello."));
        assert!(s.run("review.translate", &json!({})).is_err());
        assert!(s.run("review.translate", &json!({"to": "x"})).is_err());
        assert!(s.run("review.translate", &json!({"to": "de", "scope": "page"})).is_err());
        assert!(s.run("review.translate", &json!({"to": "de", "showIn": "elsewhere"})).is_err());
        assert!(s.run("review.translate", &json!({"to": "de", "translations": []})).is_err());
        assert!(s.run("review.translate", &json!({"to": "de", "translations": {"Hello.": 3}})).is_err());
        let e = s.run("review.translate", &json!({"to": "de"})).unwrap_err();
        assert!(e.to_string().contains("No translation model for English → German"), "{e}");
        let e = s.run("review.translate", &json!({"to": "en"})).unwrap_err();
        assert!(e.to_string().contains("already in English"), "{e}");
        // A folder without models.
        let e = s.run("review.translate", &json!({"to": "de", "model": std::env::temp_dir().to_string_lossy()})).unwrap_err();
        assert!(e.to_string().contains("No translation model"), "{e}");
        assert_eq!(s.doc.body[0].as_para().unwrap().text, "Hello.");
    }

    #[test]
    fn lang_tags_have_regions() {
        assert_eq!(lang_tag("he"), "he-IL");
        assert_eq!(lang_tag("EN"), "en-US");
        assert_eq!(lang_tag("xx"), "xx");
    }
}
