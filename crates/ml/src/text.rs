//! Cutting text into the sentences a translation model takes one at a time.

/// Characters that end a sentence when a space (or the end of the text) follows.
const ENDS: &[char] = &['.', '!', '?', '؟', '።', '।', '…'];
/// Characters that end a sentence even with no space after them (Chinese and Japanese).
const ENDS_CJK: &[char] = &['。', '！', '？'];
/// Closing quotes and brackets that stay with the sentence they end.
const CLOSERS: &[char] = &['"', '\'', '”', '’', '»', ')', ']', '」', '』', '）'];

/// The sentences of `text`, each with the spaces after it, so that joined they give back `text`.
pub fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let cjk = ENDS_CJK.contains(&c);
        if !(ENDS.contains(&c) || cjk || c == '\n') {
            continue;
        }
        let mut end = i + c.len_utf8();
        // More end marks ("?!", "..."), then closing quotes and brackets.
        while let Some(&(j, d)) = it.peek() {
            if ENDS.contains(&d) || ENDS_CJK.contains(&d) || CLOSERS.contains(&d) {
                end = j + d.len_utf8();
                it.next();
            } else {
                break;
            }
        }
        let spaced = it.peek().is_none_or(|(_, d)| d.is_whitespace());
        if !(spaced || cjk || c == '\n') {
            continue;
        }
        while let Some(&(j, d)) = it.peek() {
            if d.is_whitespace() {
                end = j + d.len_utf8();
                it.next();
            } else {
                break;
            }
        }
        if let Some(s) = text.get(start..end) {
            out.push(s);
        }
        start = end;
    }
    if let Some(rest) = text.get(start..).filter(|r| !r.is_empty()) {
        out.push(rest);
    }
    out
}

/// `text` in parts of at most `max_chars` characters (at least 1), cut after a space where there
/// is one; joined they give back `text`.
pub fn split_long(text: &str, max_chars: usize) -> Vec<&str> {
    let max_chars = max_chars.max(1);
    let mut out = Vec::new();
    let mut rest = text;
    while rest.chars().count() > max_chars {
        let limit = rest.char_indices().nth(max_chars).map_or(rest.len(), |(i, _)| i);
        let window = rest.get(..limit).unwrap_or(rest);
        let cut =
            window.char_indices().filter(|(_, c)| c.is_whitespace()).map(|(i, c)| i + c.len_utf8()).next_back().filter(|i| *i > 0).unwrap_or(limit);
        let (head, tail) = rest.split_at(cut);
        out.push(head);
        rest = tail;
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Leading whitespace, the text, trailing whitespace.
pub fn trim_parts(text: &str) -> (&str, &str, &str) {
    let core = text.trim();
    let lead = text.len() - text.trim_start().len();
    let head = text.get(..lead).unwrap_or("");
    let tail = text.get(lead + core.len()..).unwrap_or("");
    (head, core, tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_split_after_their_end_marks() {
        assert_eq!(sentences("One. Two! Three? Four"), vec!["One. ", "Two! ", "Three? ", "Four"]);
        assert_eq!(sentences("He said \"stop.\" Then left."), vec!["He said \"stop.\" ", "Then left."]);
        assert_eq!(sentences("3.14 is pi. Yes"), vec!["3.14 is pi. ", "Yes"]);
        assert_eq!(sentences("今日は。明日は。"), vec!["今日は。", "明日は。"]);
        assert_eq!(sentences("Wait... what?! ok"), vec!["Wait... ", "what?! ", "ok"]);
        assert_eq!(sentences("שלום. מה שלומך?"), vec!["שלום. ", "מה שלומך?"]);
        assert!(sentences("").is_empty());
    }

    #[test]
    fn long_text_is_cut_at_spaces() {
        assert_eq!(split_long("aaa bbb ccc", 5), vec!["aaa ", "bbb ", "ccc"]);
        assert_eq!(split_long("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(split_long("שלום עולם", 6), vec!["שלום ", "עולם"]);
        assert_eq!(split_long("x", 0), vec!["x"]);
    }

    #[test]
    fn trim_parts_keeps_the_spaces() {
        assert_eq!(trim_parts("  a b \n"), ("  ", "a b", " \n"));
        assert_eq!(trim_parts("   "), ("   ", "", ""));
        assert_eq!(trim_parts(""), ("", "", ""));
    }

    proptest::proptest! {
        #[test]
        fn pieces_join_back_to_the_text(text in "\\PC{0,80}", max in 0usize..20) {
            proptest::prop_assert_eq!(sentences(&text).concat(), text.clone());
            proptest::prop_assert_eq!(split_long(&text, max).concat(), text.clone());
            let (a, b, c) = trim_parts(&text);
            proptest::prop_assert_eq!(format!("{a}{b}{c}"), text);
        }
    }
}
