//! Run direction for mixed right-to-left and left-to-right text (#1543).
//!
//! WordCraft lays a paragraph out with the Unicode Bidirectional Algorithm (UAX #9) from its
//! text and direction alone, so a run need not say its direction. Word orders a run's characters
//! by the run's `w:rtl` (ECMA-376 Part 1 §17.3.2.30): text in a run without it is laid out left
//! to right, so Hebrew words on either side of a Latin name, and the punctuation between them,
//! change places. A run that doesn't say its direction is therefore split where the resolved
//! level changes parity, and the right-to-left pieces are written `w:rtl`, as Word writes text
//! typed in a right-to-left paragraph. Runs that say their direction, and paragraphs without
//! right-to-left characters, are written as they are.

use std::borrow::Cow;
use std::ops::Range;

use wordcraft_doc::props::CharProps;

/// `runs` (byte ranges of `text` with their props), split where the bidi level of `text` in a
/// paragraph of direction `rtl_para` changes parity; right-to-left pieces get `rtl: Some(true)`.
pub(super) fn direction_spans<'a>(
    text: &str,
    rtl_para: bool,
    runs: impl Iterator<Item = (Range<usize>, &'a CharProps)>,
) -> Vec<(Range<usize>, Cow<'a, CharProps>)> {
    if !wordcraft_doc::bidi::has_rtl(text) {
        return runs.map(|(r, p)| (r, Cow::Borrowed(p))).collect();
    }
    let level = if rtl_para { unicode_bidi::Level::rtl() } else { unicode_bidi::Level::ltr() };
    let info = unicode_bidi::ParagraphBidiInfo::new(text, Some(level));
    let odd = |at: usize| info.levels.get(at).is_some_and(|l| l.is_rtl());
    let mut out = Vec::new();
    for (range, props) in runs {
        let Some(run) = text.get(range.clone()).filter(|_| props.rtl.is_none()) else {
            out.push((range, Cow::Borrowed(props)));
            continue;
        };
        let piece = |r: Range<usize>, rtl: bool| {
            let p = if rtl { Cow::Owned(CharProps { rtl: Some(true), ..props.clone() }) } else { Cow::Borrowed(props) };
            (r, p)
        };
        let mut start = range.start;
        let mut cur: Option<bool> = None;
        for (i, _) in run.char_indices() {
            let at = range.start.saturating_add(i);
            let rtl = odd(at);
            match cur {
                Some(c) if c != rtl => {
                    out.push(piece(start..at, c));
                    start = at;
                }
                _ => {}
            }
            cur = Some(rtl);
        }
        out.push(piece(start..range.end, cur.unwrap_or(false)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_ranges_never_panic() {
        let p = CharProps::default();
        let text = "עברית WordCraft";
        let runs = vec![(0..3, &p), (3..1000, &p), (usize::MAX - 1..usize::MAX, &p), (2..text.len(), &p)];
        let out = direction_spans(text, true, runs.into_iter());
        assert_eq!(out.len(), 5, "bad ranges pass through unsplit: {out:?}");
    }
}
