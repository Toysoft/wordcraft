//! Bidirectional text for the interface (L10N-AR): `visual` turns a logical string into the order
//! egui must draw it in. The spike that chose the approach is documented below.
//!
//! **Verdict: Strategy A.** epaint 0.36.2 shapes and joins an Arabic run correctly, but it has no
//! bidi algorithm, so *runs* must be reordered by us and each run's characters left in logical
//! order. Evidence, from `epaint-0.36.2/src/text/`:
//! - `text_layout.rs::shape_text` pushes the run into a `harfrust::UnicodeBuffer` and calls
//!   `guess_segment_properties()`: direction and script come from the run's first strong
//!   character. An Arabic run is therefore shaped right-to-left with joining forms and lam-alef
//!   ligatures, and harfrust returns its glyphs in visual order. `layout_shaped_run` advances the
//!   cursor along that order, so `x` grows to the right and the first character sits rightmost.
//! - Runs are cut by *font face*, not by script (`segment_into_runs`; its doc says "would need
//!   script-aware splitting once RTL/bidi support is added", and `font.rs` has "TODO: heed bidi
//!   characters"). Runs are placed left to right in logical order, with no reordering.
//!   Consequences, observed by the probes below with Inter first and an Arabic face as fallback:
//!   * Neutrals (space, hyphen, brackets) are Inter glyphs, so they form their own runs rather
//!     than joining the neighbouring Arabic run. They keep their logical position.
//!   * Brackets are never mirrored (no `Bidi_Mirrored` handling anywhere in epaint).
//!   * Within an Arabic run, `Row::glyphs` holds the visual glyphs followed by zero-width
//!     "continuation" glyphs (`emit_continuation_glyphs`) at the run's end, so the `chr` order of
//!     a row is not the logical order. Caret and selection inside Arabic text are therefore
//!     approximate (already a non-goal for `TextEdit`).
//!   * A mixed string such as `حفظ PDF` draws the Arabic first (left) and `PDF` after it (right):
//!     plain logical order, which is wrong for an RTL base direction.
//!
//!   So Task 2 splits a string into level runs with `unicode-bidi`, mirrors paired brackets and
//!   reorders the runs (visual order), leaving the characters inside every run as they are. The
//!   probe tests pin this so an egui upgrade that changes it fails loudly. This assumes Arabic
//!   and Latin resolve to different faces; if one face covered both, a mixed run would be one
//!   buffer with a single guessed direction, so the reordering must also split runs at script
//!   changes (it does, being driven by bidi levels).

use std::borrow::Cow;
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

/// Longest string `visual` converts. Longer ones are returned as they are (an allocation cap: no
/// interface string is near this).
const MAX_LEN: usize = 64 * 1024;

const LRI: char = '\u{2066}';
const PDI: char = '\u{2069}';

/// Characters that only steer the bidi algorithm; they are never drawn, so never output.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn is_strong_rtl(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// The byte length of the `{name}` placeholder token starting at `i` (ASCII `[A-Za-z0-9_]+` in
/// braces), if there is one.
fn placeholder_len(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    if b.get(i) != Some(&b'{') {
        return None;
    }
    let mut j = i.checked_add(1)?;
    while b.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
        j = j.checked_add(1)?;
    }
    (j > i.saturating_add(1) && b.get(j) == Some(&b'}')).then(|| j.saturating_add(1).saturating_sub(i))
}

/// The direction of the first strong character (UAX #9 P2); false when there is none.
/// `{name}` placeholders are skipped: they are filled in later and are not part of the wording.
pub(crate) fn first_strong_is_rtl(s: &str) -> bool {
    let mut skip_to = 0;
    for (i, c) in s.char_indices() {
        if i < skip_to {
            continue;
        }
        if let Some(n) = placeholder_len(s, i) {
            skip_to = i.saturating_add(n);
            continue;
        }
        match bidi_class(c) {
            BidiClass::L => return false,
            BidiClass::R | BidiClass::AL => return true,
            _ => {}
        }
    }
    false
}

/// The byte length of a `` `code` `` span starting at `i` (backticks included): a command or flag
/// such as `` `--control <port>` ``, always left to right. Not when it holds a right-to-left
/// character or a line break, or runs past 256 bytes.
fn code_span_len(s: &str, i: usize) -> Option<usize> {
    let rest = s.get(i..)?.strip_prefix('`')?;
    // Look no further than the longest span: linear on text full of backticks.
    let end = rest.bytes().take(257).position(|b| b == b'`')?;
    let body = rest.get(..end)?;
    (end > 0 && end <= 256 && !body.contains('\n') && !has_rtl(body)).then(|| end.saturating_add(2))
}

/// The byte length of a file-type pattern `(*.ext)` starting at `i` (1–10 ASCII letters or
/// digits).
fn file_pattern_len(s: &str, i: usize) -> Option<usize> {
    let rest = s.get(i..)?.strip_prefix("(*.")?;
    let ext = rest.bytes().take(11).take_while(u8::is_ascii_alphanumeric).count();
    ((1..=10).contains(&ext) && rest.as_bytes().get(ext) == Some(&b')')).then(|| ext.saturating_add(4))
}

/// `s` with every left-to-right token wrapped in an LTR isolate, so it stays contiguous and
/// unchanged: `{name}` placeholders always; `` `code` `` spans and `(*.ext)` file patterns unless
/// left-to-right text comes before them (the last strong character outside an isolate is
/// left to right), where UAX #9 already keeps them with that text, as in `مستند PDF (*.pdf)` or
/// `Markdown (*.md)`. So "(*.rtf)" after an Arabic word, or at the start of a right-to-left
/// line, does not draw as "(rtf.*)". The last strong direction is carried forward in one pass:
/// linear on hostile input.
fn isolate_placeholders(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(16));
    // Was the last strong character outside an isolate left to right?
    let mut after_ltr = false;
    let mut last = 0;
    let mut i = 0;
    while let Some(c) = s.get(i..).and_then(|rest| rest.chars().next()) {
        let token = placeholder_len(s, i).or_else(|| code_span_len(s, i).or_else(|| file_pattern_len(s, i)).filter(|_| !after_ltr));
        if let Some(n) = token {
            let end = i.saturating_add(n);
            out.push_str(s.get(last..i).unwrap_or_default());
            out.push(LRI);
            out.push_str(s.get(i..end).unwrap_or_default());
            out.push(PDI);
            last = end;
            i = end;
            continue;
        }
        match bidi_class(c) {
            BidiClass::L => after_ltr = true,
            BidiClass::R | BidiClass::AL => after_ltr = false,
            _ => {}
        }
        i = i.saturating_add(c.len_utf8());
    }
    out.push_str(s.get(last..).unwrap_or_default());
    out
}

/// Whether `c` is in one of the Arabic blocks (Arabic, Supplement, Extended-A, Presentation
/// Forms A and B).
fn is_arabic_block(c: char) -> bool {
    matches!(c, '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{08A0}'..='\u{08FF}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
}

/// The paired bracket drawn for `c` in a right-to-left run.
fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        other => other,
    }
}

/// Appends an odd-level run in the order epaint must draw it: its segments reversed, each
/// strong-RTL segment in logical order (epaint shapes it right to left), brackets mirrored.
///
/// Characters in the Arabic blocks (`is_arabic_block`) join the segment before them, like
/// NSM/ZWJ/ZWNJ, even when they are neutrals (U+060C comma, U+061B, U+061F, U+066A-066C): Inter
/// lacks them, so epaint draws them with the Arabic face, inside the neighbouring Arabic run, and
/// shapes that whole run right to left. Splitting them off would put them on the wrong side. The
/// rule is by code point block, not by which face covers the character.
fn push_rtl_run(out: &mut String, run: &str) {
    // `(start, end, is_rtl_segment)` byte ranges, in logical order.
    let mut segments: Vec<(usize, usize, bool)> = Vec::new();
    for (i, c) in run.char_indices() {
        let end = i.saturating_add(c.len_utf8());
        let joins = matches!(c, '\u{200C}' | '\u{200D}') || bidi_class(c) == BidiClass::NSM;
        match segments.last_mut() {
            Some(last) if last.2 && (is_strong_rtl(c) || joins || is_arabic_block(c)) => last.1 = end,
            _ => segments.push((i, end, is_strong_rtl(c) || is_arabic_block(c))),
        }
    }
    for &(start, end, rtl) in segments.iter().rev() {
        let text = run.get(start..end).unwrap_or_default();
        if rtl {
            out.push_str(text);
        } else {
            out.extend(text.chars().map(mirrored));
        }
    }
}

/// One line (no `\n`) in draw order.
fn visual_line(line: &str, base_rtl: bool, out: &mut String) {
    let marked = isolate_placeholders(line);
    let base = if base_rtl { Level::rtl() } else { Level::ltr() };
    let info = BidiInfo::new(&marked, Some(base));
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            let text = marked.get(run.clone()).unwrap_or_default();
            let clean: String = text.chars().filter(|&c| !is_bidi_control(c)).collect();
            if levels.get(run.start).is_some_and(|l| l.is_rtl()) {
                push_rtl_run(out, &clean);
            } else {
                out.push_str(&clean);
            }
        }
    }
}

/// One text, in the order egui must draw it (epaint has no bidi algorithm; see the module doc).
///
/// `base_rtl` is the paragraph direction. Each `\n`-separated line is converted on its own.
/// Left-to-right tokens are LTR isolates, contiguous and unchanged: `{name}` placeholders, and
/// `` `code` `` spans and `(*.ext)` file patterns not preceded by left-to-right text (see
/// `isolate_placeholders`). Returns the input itself when drawing it as it stands is already
/// right: no strong right-to-left character and an LTR base, a single right-to-left run, or text
/// beyond the 64 KiB cap. The output holds no bidi control characters.
pub(crate) fn visual(s: &str, base_rtl: bool) -> Cow<'_, str> {
    if s.len() > MAX_LEN || (!base_rtl && !has_rtl(s)) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for (n, line) in s.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        visual_line(line, base_rtl, &mut out);
    }
    if out == s { Cow::Borrowed(s) } else { Cow::Owned(out) }
}

/// Does `s` have a right-to-left character or a bidi control, so drawing it needs reordering?
pub(crate) fn has_rtl(s: &str) -> bool {
    s.chars().any(|c| is_strong_rtl(c) || is_bidi_control(c))
}

/// Document or user text drawn on its own (a heading, a file name, a style name, a comment): in
/// draw order, in its own direction (its first strong character), whatever the interface
/// language. Text with no right-to-left character comes back as it is.
pub(crate) fn own(s: &str) -> Cow<'_, str> {
    visual(s, first_strong_is_rtl(s))
}

/// [`visual`] for text whose parts are styled differently (a find hit inside its context): the
/// byte ranges of `text` in the order they are drawn, each with whether its characters are
/// mirrored (brackets in a right-to-left run). As in `visual`, each `\n`-separated line (a manual
/// line break) is reordered on its own, and every `\n` is a piece of its own in place. Bidi
/// control characters stay inside the ranges; the caller drops them ([`piece_text`]). Callers may
/// split a range at their own boundaries: the characters inside one are always in logical order.
/// No token is isolated (this is for document text): `{name}` placeholders, code spans and file
/// patterns are ordinary characters here, unlike in `visual`. One range for text beyond the
/// 64 KiB cap.
pub(crate) fn visual_pieces(text: &str, base_rtl: bool) -> Vec<(std::ops::Range<usize>, bool)> {
    if text.len() > MAX_LEN || (!base_rtl && !has_rtl(text)) {
        return vec![(0..text.len(), false)];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push((start.saturating_sub(1)..start, false));
        }
        line_pieces(line, start, base_rtl, &mut out);
        start = start.saturating_add(line.len()).saturating_add(1);
    }
    out
}

/// The pieces of one line (no `\n`) starting at byte `offset` of the whole text.
fn line_pieces(line: &str, offset: usize, base_rtl: bool, out: &mut Vec<(std::ops::Range<usize>, bool)>) {
    let at = |r: std::ops::Range<usize>| offset.saturating_add(r.start)..offset.saturating_add(r.end);
    let base = if base_rtl { Level::rtl() } else { Level::ltr() };
    let info = BidiInfo::new(line, Some(base));
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for run in runs {
            if !levels.get(run.start).is_some_and(|l| l.is_rtl()) {
                out.push((at(run), false));
                continue;
            }
            // As `push_rtl_run`: segments reversed, strong-RTL ones as they are, the rest
            // mirrored. A control character never starts or ends a segment of its own kind.
            let mut segments: Vec<(usize, usize, bool)> = Vec::new();
            for (i, c) in line.get(run.clone()).unwrap_or_default().char_indices() {
                let start = run.start.saturating_add(i);
                let end = start.saturating_add(c.len_utf8());
                let control = is_bidi_control(c);
                let joins = matches!(c, '\u{200C}' | '\u{200D}') || bidi_class(c) == BidiClass::NSM;
                match segments.last_mut() {
                    Some(last) if control => last.1 = end,
                    Some(last) if last.2 && (is_strong_rtl(c) || joins || is_arabic_block(c)) => last.1 = end,
                    _ => segments.push((start, end, !control && (is_strong_rtl(c) || is_arabic_block(c)))),
                }
            }
            out.extend(segments.iter().rev().map(|&(a, b, rtl)| (at(a..b), !rtl)));
        }
    }
}

/// The text of one [`visual_pieces`] range (or part of one): bidi controls dropped, brackets
/// mirrored when the piece says so.
pub(crate) fn piece_text(s: &str, mirror: bool) -> String {
    s.chars().filter(|&c| !is_bidi_control(c)).map(|c| if mirror { mirrored(c) } else { c }).collect()
}

/// Lines of `text` (one paragraph) broken at spaces so each measures at most `width`, as byte
/// ranges in logical order; the space a line breaks at belongs to neither line. A word wider
/// than `width` sits alone on its line. Text without a word gives one empty line.
pub(crate) fn line_ranges(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start: Option<usize> = None;
    let mut end: usize = 0;
    let mut pos: usize = 0;
    for word in text.split(' ') {
        let (ws, we) = (pos, pos.saturating_add(word.len()));
        pos = we.saturating_add(1);
        if word.is_empty() {
            continue;
        }
        match start {
            None => start = Some(ws),
            Some(s) if measure(text.get(s..we).unwrap_or_default()) <= width => {}
            Some(s) => {
                lines.push(s..end);
                start = Some(ws);
            }
        }
        end = we;
    }
    let s = start.unwrap_or(0);
    lines.push(s..end.max(s));
    lines
}

/// Breaks `text` at spaces so each line measures at most `width` (by `measure`), then converts
/// every line with [`visual`]. egui would otherwise wrap the already-reordered string and put the
/// logical end of the text on top. A word wider than `width` sits alone on its line; empty text
/// gives no lines; a `width` of zero or less puts one word on each line.
pub(crate) fn wrap(text: &str, base_rtl: bool, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ').filter(|w| !w.is_empty()) {
            if line.is_empty() {
                line.push_str(word);
                continue;
            }
            let candidate = format!("{line} {word}");
            if measure(&candidate) <= width {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line.push_str(word);
            }
        }
        lines.push(line);
    }
    if text.is_empty() {
        lines.clear();
    }
    lines.iter().map(|l| visual(l, base_rtl).into_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::{first_strong_is_rtl, visual, wrap};
    use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId, RawInput};
    use std::borrow::Cow;
    use std::sync::Arc;

    /// A glyph's rectangle in the font atlas: equal for equal glyph images, wherever drawn.
    type Id = [u16; 4];

    /// An installed Arabic-capable face, if any.
    fn arabic_face() -> Option<Arc<wordcraft_fonts::FontFace>> {
        let db = wordcraft_fonts::FontDb::global();
        db.load_system_fonts();
        for family in ["Noto Sans Arabic UI", "Noto Sans Arabic", "Segoe UI", "Tahoma", "Arial", "DejaVu Sans"] {
            let face = db.face(family, "Regular");
            if face.covers('م') && face.glyph_for('ف') != 0 {
                return Some(face);
            }
        }
        None
    }

    /// A context with the interface fonts (Inter first) and the Arabic face as a fallback, as the
    /// application installs fonts (`theme::add_fallback_faces`).
    fn context() -> Option<Context> {
        // Inert without an installed Arabic face.
        let face = arabic_face()?;
        let mut defs: FontDefinitions = crate::theme::font_definitions(false, false, false);
        let mut data = FontData::from_owned(face.data().to_vec());
        data.index = face.index();
        defs.font_data.insert("probe-arabic".into(), Arc::new(data));
        if let Some(v) = defs.families.get_mut(&FontFamily::Proportional) {
            v.push("probe-arabic".into());
        }
        let ctx = Context::default();
        ctx.set_fonts(defs);
        // Fonts are applied at the start of a pass.
        ctx.run_ui(RawInput::default(), |_| {}).textures_delta.clear();
        Some(ctx)
    }

    /// `(char, x, glyph id)` of the first row, in the order epaint stores them.
    fn glyphs(ctx: &Context, text: &str) -> Vec<(char, f32, Id)> {
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap(text.into(), FontId::proportional(14.0), Color32::BLACK));
        let row = galley.rows.first().unwrap();
        row.glyphs.iter().map(|g| (g.chr, g.pos.x, [g.uv_rect.min[0], g.uv_rect.min[1], g.uv_rect.max[0], g.uv_rect.max[1]])).collect()
    }

    fn x_of(g: &[(char, f32, Id)], c: char) -> f32 {
        g.iter().find(|t| t.0 == c).unwrap_or_else(|| panic!("no glyph for {c}: {g:?}")).1
    }

    /// `visual_pieces` joined back is exactly `visual` (for text without `{name}`, code-span or
    /// file-pattern tokens), so a styled line reads as the plain one would.
    #[test]
    fn visual_pieces_join_to_visual() {
        for s in [
            "",
            "plain English (no change)",
            "مقدمة 1",
            "هذا تقرير قصير",
            "تقرير (مالي) 2024",
            "حفظ PDF الآن",
            "Report: تقرير مالي.docx",
            "a\u{200F}b مرحبا\u{2066}x\u{2069}",
            "\u{200F}مرحبا (x)",
            "مرحبا، عالم «نعم»",
            "\u{0651}\u{064E}ب",
            // A manual line break (Shift+Enter) inside a paragraph stays where it is.
            "مرحبا\nabc",
            "abc\r\nمرحبا",
            "(مهم)\nنص",
            "مرحبا\n\nنص (x)\n",
        ] {
            for base in [false, true] {
                let joined: String =
                    super::visual_pieces(s, base).into_iter().map(|(r, m)| super::piece_text(s.get(r).unwrap_or_default(), m)).collect();
                assert_eq!(joined, visual(s, base), "{s:?} base_rtl={base}");
            }
        }
    }

    #[test]
    fn line_ranges_break_at_spaces() {
        let measure = |t: &str| t.chars().count() as f32;
        let lines = |s: &'static str, w: f32| super::line_ranges(s, w, measure).into_iter().map(|r| &s[r]).collect::<Vec<_>>();
        assert_eq!(lines("aa bb cc  dd", 5.0), ["aa bb", "cc", "dd"]);
        assert_eq!(lines("averylongword x", 3.0), ["averylongword", "x"]);
        assert_eq!(lines("", 3.0), [""]);
        assert_eq!(lines("   ", 3.0), [""]);
        assert_eq!(lines("مرحبا بكم", 20.0), ["مرحبا بكم"]);
    }

    #[test]
    fn own_converts_in_the_text_s_own_direction() {
        assert!(matches!(super::own("Budget (draft)"), Cow::Borrowed("Budget (draft)")));
        assert_eq!(super::own("مقدمة 1"), visual("مقدمة 1", true));
        assert_eq!(super::own("Intro مقدمة 1"), visual("Intro مقدمة 1", false));
    }

    #[test]
    fn epaint_shapes_an_arabic_run_right_to_left() {
        let Some(ctx) = context() else { return };
        let g = glyphs(&ctx, "ملف");
        // The first letter is the rightmost: glyphs come out in visual order.
        assert!(x_of(&g, 'م') > x_of(&g, 'ف'), "{g:?}");
        assert!(x_of(&g, 'م') > x_of(&g, 'ل') && x_of(&g, 'ل') > x_of(&g, 'ف'), "{g:?}");
    }

    #[test]
    fn epaint_joins_arabic_letters() {
        let Some(ctx) = context() else { return };
        let joined = glyphs(&ctx, "ملف");
        let isolated: Vec<Id> = ["م", "ل", "ف"].iter().map(|s| glyphs(&ctx, s)[0].2).collect();
        let ids = |c: char| joined.iter().find(|t| t.0 == c).map(|t| t.2);
        // Initial meem, medial lam and final feh are different glyphs from the isolated forms.
        assert_ne!(ids('م'), Some(isolated[0]));
        assert_ne!(ids('ل'), Some(isolated[1]));
        assert_ne!(ids('ف'), Some(isolated[2]));
    }

    #[test]
    fn epaint_keeps_mixed_runs_in_logical_order() {
        let Some(ctx) = context() else { return };
        let g = glyphs(&ctx, "حفظ PDF");
        let arabic_max = ['ح', 'ف', 'ظ'].iter().map(|&c| x_of(&g, c)).fold(f32::MIN, f32::max);
        // Logical order, left to right: no base-direction reordering, so `PDF` is to the right.
        assert!(x_of(&g, 'P') > arabic_max, "P sits right of the Arabic: {g:?}");
        assert!(x_of(&g, 'P') < x_of(&g, 'D') && x_of(&g, 'D') < x_of(&g, 'F'), "{g:?}");
    }

    #[test]
    fn epaint_mirrors_parentheses_in_rtl_runs() {
        let Some(ctx) = context() else { return };
        let g = glyphs(&ctx, "(نص)");
        // Brackets come from the Latin face as runs of their own and are never mirrored: the
        // opening bracket stays leftmost.
        assert!(x_of(&g, '(') < x_of(&g, 'ن') && x_of(&g, 'ن') != x_of(&g, ')'));
        assert!(x_of(&g, '(') < x_of(&g, ')'), "{g:?}");
        let leftmost = g.iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|t| t.0);
        assert_eq!(leftmost, Some('('));
    }

    #[test]
    fn epaint_groups_neutrals() {
        let Some(ctx) = context() else { return };
        let g = glyphs(&ctx, "أ - ب");
        // Logical order, left to right: أ, space, hyphen, space, ب. The neutrals are drawn by the
        // Latin face, so they are separate runs and do not join the Arabic ones.
        let xs: Vec<f32> = g.iter().map(|t| t.1).collect();
        assert!(xs.windows(2).all(|w| w[0] <= w[1]), "strictly logical order: {g:?}");
        assert_eq!(g.iter().map(|t| t.0).collect::<String>(), "أ - ب");
    }

    #[test]
    fn ltr_text_is_borrowed_unchanged() {
        assert!(matches!(visual("Save As…", false), Cow::Borrowed("Save As…")));
        assert!(matches!(visual("PDF", true), Cow::Borrowed("PDF")));
    }

    #[test]
    fn a_single_arabic_run_is_unchanged() {
        assert_eq!(visual("ملف", true), "ملف");
        assert!(matches!(visual("ملف", true), Cow::Borrowed(_)));
    }

    #[test]
    fn runs_are_reordered_for_an_rtl_paragraph() {
        // Logical "حفظ PDF": the Arabic word is drawn to the right of "PDF", the space between them.
        assert_eq!(visual("حفظ PDF", true), "PDF حفظ");
    }

    #[test]
    fn placeholders_stay_whole_and_take_their_visual_slot() {
        assert_eq!(visual("صفحة {page} من {pages}", true), "{pages} من {page} صفحة");
    }

    /// Code spans and file patterns are left-to-right tokens: without the isolate, UAX #9 gives the
    /// neutrals at their edges the Arabic direction, so "(*.rtf)" drew as "(rtf.*)" and
    /// "`--control <port>`" as "`<control <port--`" (seen in the Task 16 screenshots).
    #[test]
    fn code_spans_and_file_patterns_stay_left_to_right() {
        assert_eq!(visual("تنسيق النص الغني (*.rtf)", true), "(*.rtf) الغني النص تنسيق");
        assert_eq!(visual("مستند PDF (*.pdf)", true), "PDF (*.pdf) مستند");
        assert_eq!(visual("Markdown (*.md)", true), "Markdown (*.md)");
        // At the start of a right-to-left line, too (nothing left to right before them).
        assert_eq!(visual("(*.md) ملف", true), "ملف (*.md)");
        assert_eq!(visual("`--x` ملف", true), "ملف `--x`");
        // After a placeholder (an isolate, not left-to-right text of the line).
        assert_eq!(visual("{n} (*.md)", true), "(*.md) {n}");
        assert_eq!(visual("ابدأ مع `--control <port>` الآن", true), "الآن `--control <port>` مع ابدأ");
        // Not tokens: an unclosed backtick, a pattern without an extension, Arabic in a span.
        assert_eq!(visual("أ `--x", true), "x--` أ");
        assert_eq!(visual("أ (*.)", true), "(.*) أ");
        assert_eq!(visual("أ `ب`", true), "`ب` أ");
        // Left-to-right text is untouched.
        assert!(matches!(visual("Rich Text Format (*.rtf)", false), Cow::Borrowed(_)));
    }

    #[test]
    fn every_line_is_reordered_on_its_own() {
        assert_eq!(visual("حفظ PDF\nملف", true), "PDF حفظ\nملف");
    }

    #[test]
    fn neutrals_and_brackets_in_rtl_runs_are_ordered_and_mirrored() {
        assert_eq!(visual("نص (مثال)", true), "(مثال) نص");
        assert_eq!(visual("الكلمات: 21", true), "21 :الكلمات");
    }

    #[test]
    fn output_has_no_bidi_controls() {
        for s in ["\u{202E}abc", "x\u{200F}y", "{a} ب", "a\u{2067}b"] {
            for rtl in [true, false] {
                let out = visual(s, rtl);
                let is_control = |c: char| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}');
                assert!(!out.chars().any(is_control), "{s:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn hostile_input_never_panics() {
        for s in ["\u{202E}", "\u{2067}{", "{", "}{", "{a", "\u{0}", &"ب".repeat(100_000), "a\u{2069}\u{2069}", "\u{2029}ب\r\u{85}"] {
            let _ = visual(s, true);
            let _ = visual(s, false);
        }
    }

    /// Many code spans or file patterns near the 64 KiB cap stay linear: the token check used to
    /// rescan the whole prefix for its last strong character (quadratic, every frame).
    #[test]
    fn many_tokens_near_the_cap_stay_fast() {
        let spans = format!("{}ا", "`1` ".repeat(16_000));
        let patterns = format!("ا{}", " (*.a)".repeat(10_000));
        let started = std::time::Instant::now();
        for s in [&spans, &patterns] {
            assert!(s.len() < super::MAX_LEN);
            let _ = visual(s, true);
            let _ = visual(s, false);
            let _ = super::visual_pieces(s, true);
        }
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(2), "took {took:?}");
    }

    #[test]
    fn arabic_block_neutrals_stay_with_their_arabic_segment() {
        // The Arabic comma is drawn by the Arabic face inside the run of the word before it.
        assert_eq!(visual("نعم، لا", true), "لا نعم،");
    }

    #[test]
    fn epaint_draws_the_arabic_comma_left_of_its_word() {
        let Some(ctx) = context() else { return };
        let g = glyphs(&ctx, &visual("نعم، لا", true));
        let xs = |cs: &[char]| g.iter().filter(|t| cs.contains(&t.0)).map(|t| t.1).collect::<Vec<f32>>();
        let lam_alef_max = xs(&['ل', 'ا']).into_iter().fold(f32::MIN, f32::max);
        let word_min = xs(&['ن', 'ع', 'م']).into_iter().fold(f32::MAX, f32::min);
        let comma = x_of(&g, '،');
        assert!(lam_alef_max < comma && comma < word_min, "{g:?}");
    }

    #[test]
    fn first_strong_direction() {
        assert!(first_strong_is_rtl("123 ملف"));
        assert!(!first_strong_is_rtl("report ملف"));
        assert!(!first_strong_is_rtl("123"));
        // A placeholder's name is not a strong character of the string.
        assert!(first_strong_is_rtl("{count} ملف"));
    }

    #[test]
    fn wrap_breaks_logically_then_reorders_each_line() {
        let m = |s: &str| s.chars().count() as f32;
        let lines = wrap("حفظ الملف بتنسيق PDF الآن", true, 16.0, m);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0], visual("حفظ الملف بتنسيق", true));
        assert_eq!(lines[1], visual("PDF الآن", true));
    }

    #[test]
    fn wrap_survives_degenerate_input() {
        let m = |s: &str| s.chars().count() as f32;
        assert!(wrap("", true, 10.0, m).is_empty());
        assert_eq!(wrap("a bb ccc", false, 0.0, m), ["a", "bb", "ccc"]);
        assert_eq!(wrap("a bb ccc", false, -5.0, m), ["a", "bb", "ccc"]);
        assert_eq!(wrap("a verylongword b", false, 4.0, m), ["a", "verylongword", "b"]);
        assert_eq!(
            wrap(
                "a

b", false, 10.0, m
            ),
            ["a", "", "b"]
        );
        assert_eq!(wrap("a b", false, f32::NAN, m), ["a", "b"]);
    }
}
