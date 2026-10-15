//! Language codes for the models: the two-letter ISO 639-1 codes WordCraft uses, from the
//! three-letter ones some model names carry, and a guess of a text's language from its script.

/// The languages offered for dictation and as translation targets: (code, English name).
/// Whisper hears all of these; translation needs a model for the pair.
pub const LANGUAGES: [(&str, &str); 37] = [
    ("ar", "Arabic"),
    ("bg", "Bulgarian"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("nl", "Dutch"),
    ("no", "Norwegian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sr", "Serbian"),
    ("sv", "Swedish"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("vi", "Vietnamese"),
    ("zh", "Chinese"),
];

/// Languages written right to left.
pub fn is_rtl(code: &str) -> bool {
    matches!(normalize(code).as_str(), "ar" | "he" | "fa" | "ur" | "yi" | "ps" | "sd" | "ug" | "dv")
}

/// Three-letter codes (ISO 639-2/3) to two-letter ones, for the codes model names use.
const THREE: &[(&str, &str)] = &[
    ("ara", "ar"),
    ("bul", "bg"),
    ("cat", "ca"),
    ("ces", "cs"),
    ("cze", "cs"),
    ("dan", "da"),
    ("deu", "de"),
    ("ger", "de"),
    ("ell", "el"),
    ("gre", "el"),
    ("eng", "en"),
    ("spa", "es"),
    ("est", "et"),
    ("fas", "fa"),
    ("per", "fa"),
    ("fin", "fi"),
    ("fra", "fr"),
    ("fre", "fr"),
    ("heb", "he"),
    ("hin", "hi"),
    ("hrv", "hr"),
    ("hun", "hu"),
    ("ind", "id"),
    ("ita", "it"),
    ("jpn", "ja"),
    ("kor", "ko"),
    ("lit", "lt"),
    ("lav", "lv"),
    ("nld", "nl"),
    ("dut", "nl"),
    ("nor", "no"),
    ("nob", "no"),
    ("pol", "pl"),
    ("por", "pt"),
    ("ron", "ro"),
    ("rum", "ro"),
    ("rus", "ru"),
    ("slk", "sk"),
    ("slo", "sk"),
    ("srp", "sr"),
    ("swe", "sv"),
    ("tha", "th"),
    ("tur", "tr"),
    ("ukr", "uk"),
    ("vie", "vi"),
    ("zho", "zh"),
    ("chi", "zh"),
];

/// A language tag reduced to its lowercase primary code, two letters where one exists:
/// `he-IL` → `he`, `heb` → `he`, `iw` (Hebrew's old code) → `he`, `nb` → `no`.
pub fn normalize(tag: &str) -> String {
    let primary = tag.trim().split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    match primary.as_str() {
        "iw" => "he".into(),
        "nb" | "nn" => "no".into(),
        "in" => "id".into(),
        p => THREE.iter().find(|(three, _)| *three == p).map_or(primary.clone(), |(_, two)| (*two).to_string()),
    }
}

/// The English name of a language code, or the code itself.
pub fn name(code: &str) -> String {
    let c = normalize(code);
    LANGUAGES.iter().find(|(k, _)| *k == c).map_or(c.clone(), |(_, n)| (*n).to_string())
}

/// A guess of the language of `text` from the script most of its letters are in, for scripts
/// that belong to one language (Hebrew, Greek, Thai, Korean, Japanese kana…); `None` for Latin
/// and Cyrillic text and text without letters, which the caller decides otherwise.
pub fn guess_from_script(text: &str) -> Option<&'static str> {
    let mut counts = [0usize; 8];
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        let slot = match c as u32 {
            0x0590..=0x05FF | 0xFB1D..=0xFB4F => 0,
            0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => 1,
            0x0370..=0x03FF => 2,
            0x0E00..=0x0E7F => 3,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => 4,
            0x3040..=0x30FF => 5,
            0x4E00..=0x9FFF => 6,
            0x0900..=0x097F => 7,
            _ => continue,
        };
        if let Some(n) = counts.get_mut(slot) {
            *n += 1;
        }
    }
    let total: usize = text.chars().filter(|c| c.is_alphabetic()).count();
    let (slot, n) = counts.iter().enumerate().max_by_key(|(_, n)| **n)?;
    if *n == 0 || *n * 2 < total {
        return None;
    }
    // Kana anywhere makes Han text Japanese.
    if slot == 6 && counts.get(5).is_some_and(|k| *k > 0) {
        return Some("ja");
    }
    ["he", "ar", "el", "th", "ko", "ja", "zh", "hi"].get(slot).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_normalize_to_two_letter_codes() {
        assert_eq!(normalize("he-IL"), "he");
        assert_eq!(normalize("heb"), "he");
        assert_eq!(normalize("iw"), "he");
        assert_eq!(normalize("EN_us"), "en");
        assert_eq!(normalize("nb-NO"), "no");
        assert_eq!(normalize("mul"), "mul");
        assert_eq!(normalize(""), "");
        assert_eq!(name("de-AT"), "German");
        assert_eq!(name("xx"), "xx");
        assert!(is_rtl("he") && is_rtl("ar-EG") && !is_rtl("en"));
    }

    #[test]
    fn scripts_suggest_languages() {
        assert_eq!(guess_from_script("שלום עולם, hello"), Some("he"));
        assert_eq!(guess_from_script("مرحبا"), Some("ar"));
        assert_eq!(guess_from_script("こんにちは世界"), Some("ja"));
        assert_eq!(guess_from_script("你好世界"), Some("zh"));
        assert_eq!(guess_from_script("안녕하세요"), Some("ko"));
        assert_eq!(guess_from_script("Hello world"), None);
        assert_eq!(guess_from_script("Привет"), None);
        assert_eq!(guess_from_script("123"), None);
    }
}
