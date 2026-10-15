//! Greek spelling using an optional, locally installed Hunspell `el_GR` dictionary.
//!
//! No dictionary is shipped with WordCraft: it discovers `el_GR.aff`/`el_GR.dic` via
//! the same search paths as other supported languages. Without the dictionary,
//! no Greek words are flagged. Ancient Greek/polytonic text is left untouched.
//! This module implements spelling only; it makes no claim of Greek grammar checking.

use std::sync::{Arc, RwLock};

use crate::affix::AffixDict;

static DICT: RwLock<Option<Arc<AffixDict>>> = RwLock::new(None);
const NAMES: &[&str] = &["el_GR", "el-GR", "el"];

pub fn dictionary() -> Arc<AffixDict> {
    if let Some(d) = DICT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return d.clone();
    }
    let loaded = Arc::new(load_installed());
    let mut slot = DICT.write().unwrap_or_else(|e| e.into_inner());
    slot.get_or_insert(loaded).clone()
}

fn load_installed() -> AffixDict {
    let Some((aff, dic)) = crate::hunspell::find(NAMES) else {
        log::info!("Greek spelling: no el_GR Hunspell dictionary installed");
        return AffixDict::default();
    };
    crate::hunspell::load(&aff, &dic).unwrap_or_else(|e| {
        log::warn!("Greek spelling: {e}");
        AffixDict::default()
    })
}

/// Supply decoded UTF-8 Hunspell data; an invalid dictionary leaves the previous one intact.
pub fn install_dictionary(aff: &str, dic: &str) -> Result<(), String> {
    let d = AffixDict::from_hunspell(aff, dic, &[])?;
    *DICT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(d));
    Ok(())
}

pub fn reload_dictionary() {
    *DICT.write().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn installed_path() -> Option<std::path::PathBuf> {
    crate::hunspell::find(NAMES).map(|(_, dic)| dic)
}

pub fn has_dictionary() -> bool {
    !dictionary().is_empty()
}

fn is_greek(c: char) -> bool {
    ('\u{0370}'..='\u{03ff}').contains(&c) || ('\u{1f00}'..='\u{1fff}').contains(&c)
}

fn is_polytonic(c: char) -> bool {
    ('\u{1f00}'..='\u{1fff}').contains(&c)
}

/// In an unavailable dictionary all words pass; foreign-script, polytonic and numeric
/// tokens are not accidentally treated as errors in a Modern Greek document.
pub fn is_correct(word: &str) -> bool {
    is_correct_with(&dictionary(), word)
}

pub fn is_correct_with(d: &AffixDict, word: &str) -> bool {
    if d.is_empty() {
        return true;
    }
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’');
    let w = w.trim_matches(['\'', '’']);
    let n = w.chars().count();
    if n <= 1 || w.chars().any(char::is_numeric) {
        return true;
    }
    if w.chars().all(|c| !c.is_lowercase()) && n <= 6 {
        return true; // abbreviations
    }
    if !w.chars().any(is_greek) || w.chars().any(is_polytonic) {
        return true; // leave foreign or Ancient Greek text alone
    }
    if w.chars().any(|c| c.is_alphabetic() && !is_greek(c)) {
        return true; // names and mixed-script tokens
    }
    let normalized = w.replace('’', "'");
    if normalized.contains('-') {
        return normalized.split('-').all(|part| known(d, part));
    }
    known(d, &normalized)
}

fn known(d: &AffixDict, word: &str) -> bool {
    word.chars().count() <= 1 || d.check(word) || crate::in_user_dictionary(&word.to_lowercase())
}

/// Bounded dictionary-checked suggestions. Accents first, then Hunspell REP
/// confusions (e.g. ει/ι), transposition, insertion, deletion and substitution.
/// All proposals must be valid dictionary forms. No online services are used.
pub fn suggest(word: &str, max: usize) -> Vec<String> {
    suggest_with(&dictionary(), word, max)
}

pub fn suggest_with(d: &AffixDict, word: &str, max: usize) -> Vec<String> {
    let len = word.chars().count();
    if max == 0 || d.is_empty() || !(2..=40).contains(&len) || !word.chars().all(is_greek) || word.chars().any(is_polytonic) {
        return Vec::new();
    }
    let original = word.to_lowercase();
    let chars: Vec<char> = original.chars().collect();
    let mut found = Vec::new();
    let cap = max.min(10);
    let initial_uppercase = word.chars().next().is_some_and(char::is_uppercase);
    // Candidate spelling is validated after restoring a proper name's initial capital.
    // A dictionary containing only «Ελλάδα» must still suggest it for «Ελλαδα».
    let mut offer = |candidate: String| {
        if found.len() >= cap || candidate == original {
            return;
        }
        let shown = if initial_uppercase {
            let mut chars = candidate.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().to_string() + chars.as_str(),
                None => return,
            }
        } else {
            candidate
        };
        if d.check(&shown) && !found.contains(&shown) {
            found.push(shown);
        }
    };
    // Common error: missing or misplaced tonos; the orthography is dictionary-checked.
    for (i, &c) in chars.iter().enumerate() {
        let alternatives: &str = match c {
            'α' => "ά",
            'ε' => "έ",
            'η' => "ή",
            'ι' => "ίϊΐ",
            'ο' => "ό",
            'υ' => "ύϋΰ",
            'ω' => "ώ",
            'ά' => "α",
            'έ' => "ε",
            'ή' => "η",
            'ί' | 'ϊ' | 'ΐ' => "ι",
            'ό' => "ο",
            'ύ' | 'ϋ' | 'ΰ' => "υ",
            'ώ' => "ω",
            _ => "",
        };
        for repl in alternatives.chars() {
            let mut candidate = chars.clone();
            candidate[i] = repl;
            offer(candidate.iter().collect());
        }
    }
    // Hunspell's Greek REP table covers two-letter sound-alike sequences that a
    // single-letter edit cannot fix (e.g. «κίμενο» → «κείμενο»). Limit the
    // number of rules examined; the installed file has hundreds.
    for (wrong, right) in d.rep().iter().take(600) {
        if wrong.is_empty() || right.is_empty() || wrong.contains(' ') || right.contains(' ') {
            continue;
        }
        for (start, _) in original.match_indices(wrong.as_str()) {
            let mut candidate = String::with_capacity(original.len() + right.len());
            candidate.push_str(&original[..start]);
            candidate.push_str(right);
            candidate.push_str(&original[start + wrong.len()..]);
            offer(candidate);
        }
    }
    for i in 0..chars.len().saturating_sub(1) {
        let mut candidate = chars.clone();
        candidate.swap(i, i + 1);
        offer(candidate.iter().collect());
    }
    for i in 0..chars.len() {
        let mut candidate = chars.clone();
        candidate.remove(i);
        offer(candidate.iter().collect());
    }
    // Include final sigma: an accidental word-final «σ» can become «ς».
    const LETTERS: &str = "αβγδεζηθικλμνξοπρσςτυφχψω";
    for i in 0..chars.len() {
        for repl in LETTERS.chars() {
            if repl == chars[i] {
                continue;
            }
            let mut candidate = chars.clone();
            candidate[i] = repl;
            offer(candidate.iter().collect());
        }
    }
    for i in 0..=chars.len() {
        for repl in LETTERS.chars() {
            let mut candidate = chars.clone();
            candidate.insert(i, repl);
            offer(candidate.iter().collect());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AffixDict {
        AffixDict::from_hunspell(
            "SET UTF-8\nTRY αβγδεζηθικλμνξοπρστυφχψω\nREP 1\nREP ί εί\n",
            "7\nκαλημέρα\nκόσμος\nπρόγραμμα\nΕλλάδα\nσήμερα\nκείμενο\nλόγος\n",
            &[],
        )
        .unwrap()
    }

    #[test]
    fn correct_and_incorrect_modern_greek() {
        let d = sample();
        for good in ["καλημέρα", "κόσμος", "πρόγραμμα", "Καλημέρα", "WordCraft", "ἄνθρωπος", "123"] {
            assert!(is_correct_with(&d, good), "{good}");
        }
        for bad in ["καλημερα", "κοσμος", "προγραμμα", "καλημέρρα"] {
            assert!(!is_correct_with(&d, bad), "{bad}");
        }
        assert!(is_correct_with(&AffixDict::default(), "καλημερα"));
    }

    #[test]
    fn missing_accent_is_suggested() {
        let d = sample();
        assert!(suggest_with(&d, "καλημερα", 5).contains(&"καλημέρα".to_owned()));
        assert!(suggest_with(&d, "κοσμος", 5).contains(&"κόσμος".to_owned()));
        assert!(suggest_with(&d, "κόσμος", 0).is_empty());
    }

    #[test]
    fn proper_names_preserve_capitalization() {
        let d = sample();
        assert!(is_correct_with(&d, "Ελλάδα"));
        assert!(!is_correct_with(&d, "Ελλαδα"));
        assert!(suggest_with(&d, "Ελλαδα", 5).contains(&"Ελλάδα".to_owned()));
        assert!(!is_correct_with(&d, "ελλάδα"), "capitalized dictionary names are not common nouns");
    }

    #[test]
    fn greek_rep_and_final_sigma_suggestions() {
        let d = sample();
        assert!(suggest_with(&d, "κίμενο", 5).contains(&"κείμενο".to_owned()));
        assert!(suggest_with(&d, "λόγοσ", 10).contains(&"λόγος".to_owned()));
        assert!(!is_correct_with(&d, "λόγοσ"));
    }

    #[test]
    fn missing_dictionary_and_ancient_greek_are_safe() {
        let d = AffixDict::default();
        assert!(is_correct_with(&d, "καλημερα"));
        assert!(suggest_with(&d, "καλημερα", 5).is_empty());
        assert!(is_correct_with(&sample(), "ἄνθρωπος"));
        assert!(suggest_with(&sample(), "ἄνθρωποσ", 5).is_empty());
    }
}
