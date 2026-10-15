//! English full-form spelling lexicon, independent of the hyphenation dictionary.
//!
//! ESDB supplies explicit inflections rather than licensing arbitrary suffixes on every stem.
//! The immutable index borrows the bundled UTF-8 data; no file access or affix expansion is needed.
use std::borrow::Cow;
use std::sync::OnceLock;

const FORMS: &str = include_str!("../../../assets/spelling/en-forms.txt");
pub(crate) const EXTRA: &str = include_str!("../../../assets/spelling/en-extra.txt");
const MAX_SUGGEST_CHARS: usize = 40;

struct Entry {
    word: &'static str,
    suggest: bool,
    lower: Cow<'static, str>,
}

fn entry(word: &'static str, suggest: bool) -> Entry {
    let lower = if word.chars().any(char::is_uppercase) { Cow::Owned(word.to_lowercase()) } else { Cow::Borrowed(word) };
    Entry { word, suggest, lower }
}

struct Lexicon {
    entries: Vec<Entry>,
    lengths: [Vec<usize>; MAX_SUGGEST_CHARS + 1],
}

fn lexicon() -> &'static Lexicon {
    static LEXICON: OnceLock<Lexicon> = OnceLock::new();
    LEXICON.get_or_init(|| {
        let mut entries: Vec<Entry> = FORMS
            .lines()
            .map(|line| {
                let (word, suggest) = line.strip_suffix("\t!").map_or((line, true), |w| (w, false));
                entry(word, suggest)
            })
            .chain(EXTRA.lines().filter(|w| !w.is_empty() && !w.starts_with('#')).map(|word| entry(word, true)))
            .collect();
        entries.sort_by_key(|e| e.word);
        // Never re-enable suggestions for a source entry marked NOSUGGEST.
        entries.dedup_by(|b, a| {
            if a.word == b.word {
                a.suggest &= b.suggest;
                true
            } else {
                false
            }
        });
        let mut lengths: [Vec<usize>; MAX_SUGGEST_CHARS + 1] = std::array::from_fn(|_| Vec::new());
        for (i, e) in entries.iter().enumerate() {
            if e.suggest
                && let Some(bucket) = lengths.get_mut(e.word.chars().count())
            {
                bucket.push(i);
            }
        }
        Lexicon { entries, lengths }
    })
}

fn exact(word: &str) -> bool {
    lexicon().entries.binary_search_by_key(&word, |e| e.word).is_ok()
}

pub(crate) fn contains(word: &str) -> bool {
    if exact(word) {
        return true;
    }
    match crate::affix::casing(word) {
        crate::affix::Casing::Initial => exact(&word.to_lowercase()),
        crate::affix::Casing::Upper => {
            let lower = word.to_lowercase();
            exact(&lower) || exact(&crate::affix::capitalize(&lower))
        }
        _ => false,
    }
}

pub(crate) fn candidates(chars: usize) -> impl Iterator<Item = (&'static str, &'static str)> {
    let lex = lexicon();
    lex.lengths
        .iter()
        .enumerate()
        .filter(move |(n, _)| n.abs_diff(chars) <= 2)
        .flat_map(|(_, indices)| indices.iter())
        .filter_map(|&i| lex.entries.get(i))
        .map(|e| (e.word, e.lower.as_ref()))
}

/// Bounded optimal string alignment distance. Scratch rows are reused across candidates.
pub(crate) struct Distance {
    rows: [[usize; MAX_SUGGEST_CHARS + 1]; 3],
}

impl Distance {
    pub(crate) fn new() -> Self {
        Self { rows: [[0; MAX_SUGGEST_CHARS + 1]; 3] }
    }

    pub(crate) fn score(&mut self, a: &[char], b: &str) -> usize {
        let mut target = ['\0'; MAX_SUGGEST_CHARS];
        let mut m = 0;
        for c in b.chars() {
            let Some(slot) = target.get_mut(m) else { return 3 };
            *slot = c;
            m += 1;
        }
        if a.len() > MAX_SUGGEST_CHARS || a.len().abs_diff(m) > 2 {
            return 3;
        }
        for j in 0..=m {
            self.rows[0][j] = j;
        }
        let (mut prev2, mut prev, mut cur) = (1, 0, 2);
        for i in 1..=a.len() {
            self.rows[cur][0] = i;
            let mut min = i;
            for j in 1..=m {
                let cost = usize::from(a.get(i - 1) != target.get(j - 1));
                let mut v = (self.rows[prev][j] + 1).min(self.rows[cur][j - 1] + 1).min(self.rows[prev][j - 1] + cost);
                if i > 1 && j > 1 && a.get(i - 1) == target.get(j - 2) && a.get(i - 2) == target.get(j - 1) {
                    v = v.min(self.rows[prev2][j - 2] + 1);
                }
                self.rows[cur][j] = v;
                min = min.min(v);
            }
            if min > 2 {
                return 3;
            }
            (prev2, prev, cur) = (prev, cur, prev2);
        }
        self.rows[prev][m]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_sorted_unique_and_has_no_unexpected_fields() {
        let words: Vec<_> = FORMS.lines().map(|l| l.strip_suffix("\t!").unwrap_or(l)).collect();
        assert!(words.len() > 100_000);
        assert!(words.windows(2).all(|w| w[0] < w[1]));
        assert!(words.iter().all(|w| !w.is_empty() && !w.contains(['\t', ' '])));
    }

    #[test]
    fn inflections_require_lexical_evidence() {
        for w in ["bought", "ate", "went", "children", "running", "stopped", "happiest", "happily", "specifically", "downloaded", "uploading"] {
            assert!(contains(w), "{w}");
        }
        for w in ["buyed", "eated", "goed", "childs", "catest", "catly", "catness", "miscat", "unhello"] {
            assert!(!contains(w), "{w}");
        }
    }

    #[test]
    fn proper_names_and_sentence_case_remain_distinct() {
        assert!(contains("London") && contains("LONDON"));
        assert!(!contains("london"));
        assert!(contains("Bought") && contains("BOUGHT"));
        assert!(!contains("bOuGhT"));
    }

    #[test]
    fn distance_reuse_covers_close_matches_and_transpositions() {
        let mut distance = Distance::new();
        for (a, b, expected) in [
            ("recieve", "receive", 1),
            ("ab", "ba", 1),
            ("world", "wrld", 1),
            ("wrld", "world", 1),
            ("speling", "spelling", 1),
            ("cat", "cat", 0),
            ("cat", "cats", 1),
            ("a", "", 1),
            ("", "a", 1),
            ("", "", 0),
            ("café", "cafe", 1),
            ("qzx", "abc", 3),
        ] {
            assert_eq!(distance.score(&a.chars().collect::<Vec<_>>(), b), expected, "{a} -> {b}");
        }
    }

    #[test]
    fn suggestions_are_source_words_and_respect_nosuggest() {
        for n in 0..=40 {
            for (w, _) in candidates(n) {
                assert!(w.chars().count().abs_diff(n) <= 2);
                let i = lexicon().entries.binary_search_by_key(&w, |e| e.word).unwrap();
                assert!(lexicon().entries[i].suggest);
            }
        }
    }

    proptest::proptest! {
        #[test]
        fn reused_distance_matches_reference(a in "[a-zé]{0,40}", bs in proptest::collection::vec("[a-zé]{0,40}", 1..8)) {
            let a: Vec<_> = a.chars().collect();
            let mut distance = Distance::new();
            for b in bs {
                proptest::prop_assert_eq!(distance.score(&a, &b).min(3), crate::damerau(&a, &b).min(3));
            }
        }
    }
}
