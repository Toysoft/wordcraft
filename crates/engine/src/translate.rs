//! Translate (Review › Language): every text of a document — body, tables, headers and footers,
//! footnotes and endnotes, text boxes and comments — in another language, with its design kept.
//!
//! Paragraphs, their styles and properties, lists, tables, pictures, shapes, fields and sections
//! stay as they are: only text changes. A paragraph's text is cut into *chunks* at what must stay
//! put (pictures, shapes, fields, note references, tabs, line and page breaks, tracked deletions,
//! citations); each chunk is translated as one piece, so a sentence keeps its context, and its
//! formatting is mapped back onto the translation:
//!
//! - The formatting most of the chunk's letters have becomes the translation's.
//! - Each differently formatted stretch (a bold word, a link, a coloured phrase) is translated on
//!   its own too and looked for in the translation; where it is found, that text takes its
//!   formatting. Where it isn't (the model worded it differently), the formatting goes to the
//!   words at the same relative place.
//! - Bookmarks, comment anchors and editing-permission marks inside a chunk move to the word
//!   boundary nearest their relative place, in their order.
//!
//! The translating itself is done elsewhere: a [`Plan`] lists the texts to translate
//! ([`Plan::texts`]) and applies a text → translation map ([`Plan::apply`]). `review.translate`
//! fills the map from a local model (`wordcraft-ml`) or takes it from its caller — the app runs
//! the model on a worker thread and hands the map over, so the window never waits.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use wordcraft_doc::para::{COLUMN_BREAK, LINE_BREAK, OBJ, PAGE_BREAK, Run};
use wordcraft_doc::props::CharProps;
use wordcraft_doc::{Document, InlineObject, Path, StoryRef};

/// Most paragraphs one plan covers.
const MAX_PARAS: usize = 200_000;
/// Most texts one plan asks to translate.
pub const MAX_TEXTS: usize = 200_000;
/// Most differently formatted stretches of one chunk translated on their own and looked for in
/// its translation; the rest (in a hostile paragraph of thousands of runs) are placed by position.
const MAX_SPAN_LOOKUPS: usize = 32;
/// Most spans whose formatting is grouped to find the chunk's main formatting; past it each span
/// counts alone (grouping is quadratic).
const MAX_GROUPED_SPANS: usize = 64;
/// Most markers in a chunk placed at word boundaries; past it they go to their proportional place.
const MAX_SNAPPED_MARKERS: usize = 64;

/// What to translate.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    /// Every story of the document.
    Document,
    /// These paragraphs.
    Paragraphs(Vec<(StoryRef, Path)>),
}

/// A formatted stretch of a chunk's text.
#[derive(Clone, Debug, PartialEq)]
struct Span {
    /// Byte range in the chunk's text.
    range: Range<usize>,
    props: CharProps,
}

/// A stretch of a paragraph translated as one piece.
#[derive(Clone, Debug, PartialEq)]
struct Chunk {
    /// Byte range in the paragraph's text, markers included.
    range: Range<usize>,
    /// The text without its markers.
    text: String,
    spans: Vec<Span>,
    /// Markers (bookmarks, comment anchors…): byte offset in `text`, and the marker's own formatting.
    markers: Vec<(usize, CharProps)>,
}

impl Chunk {
    /// The span whose formatting most letters have.
    fn base(&self) -> usize {
        let letters_in = |o: &Span| self.text.get(o.range.clone()).unwrap_or("").chars().filter(|c| c.is_alphabetic()).count();
        let grouped = self.spans.len() <= MAX_GROUPED_SPANS;
        let mut best = (0usize, 0usize);
        for (i, s) in self.spans.iter().enumerate() {
            let letters: usize = if grouped { self.spans.iter().filter(|o| o.props == s.props).map(letters_in).sum() } else { letters_in(s) };
            if letters > best.1 {
                best = (i, letters);
            }
        }
        best.0
    }

    /// The texts this chunk needs translated: its whole text, then each differently formatted
    /// stretch with letters in it.
    fn requests(&self) -> Vec<String> {
        let core = self.text.trim();
        let mut out = vec![core.to_string()];
        if let Some(base) = self.spans.get(self.base()) {
            let pieces = self
                .spans
                .iter()
                .filter(|s| s.props != base.props)
                .map(|s| self.text.get(s.range.clone()).unwrap_or("").trim())
                .filter(|t| t.chars().any(char::is_alphabetic) && *t != core)
                .take(MAX_SPAN_LOOKUPS);
            out.extend(pieces.map(str::to_string));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ParaPlan {
    story: StoryRef,
    path: Path,
    chunks: Vec<Chunk>,
}

/// The texts of a document to translate, and where they go back.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Plan {
    paras: Vec<ParaPlan>,
}

/// What [`Plan::apply`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Paragraphs whose text changed.
    pub paragraphs: usize,
    /// Chunks translated.
    pub translated: usize,
    /// Chunks left as they were because the map had no translation for them.
    pub untranslated: usize,
}

/// Options for [`Plan::apply`].
#[derive(Clone, Debug, PartialEq)]
pub struct ApplyOptions {
    /// Target language (BCP 47): translated text is marked as written in it, and paragraphs
    /// translated into a right-to-left language read right to left.
    pub to: String,
}

/// Zero-width markers that move with the text around them.
fn is_marker(o: &InlineObject) -> bool {
    matches!(
        o,
        InlineObject::BookmarkStart { .. }
            | InlineObject::BookmarkEnd { .. }
            | InlineObject::CommentStart { .. }
            | InlineObject::CommentEnd { .. }
            | InlineObject::PermStart { .. }
            | InlineObject::PermEnd { .. }
    )
}

/// Every story of the document: the body, then its parts (headers, footers, notes, comments,
/// text boxes).
fn stories(doc: &Document) -> Vec<StoryRef> {
    std::iter::once(StoryRef::Body).chain(doc.parts.keys().map(|k| StoryRef::Part(*k))).collect()
}

/// Byte ranges of each paragraph (by path) that citation and bibliography fields cover: their
/// text is generated by the citation manager and stays as it is.
fn citation_spans(doc: &Document, story: StoryRef) -> HashMap<Path, Vec<Range<usize>>> {
    let mut out: HashMap<Path, Vec<Range<usize>>> = HashMap::new();
    let ranges: Vec<_> = doc.field_ranges(story).into_iter().filter(|r| crate::speech::is_citation_code(&r.instr)).collect();
    if ranges.is_empty() {
        return out;
    }
    for path in doc.para_paths(story) {
        let Some(len) = doc.para(story, &path).map(|p| p.text.len()) else { continue };
        for r in &ranges {
            if path < r.start.path || path > r.end.path {
                continue;
            }
            let a = if path == r.start.path { r.start.off } else { 0 };
            let b = if path == r.end.path { r.end.off.saturating_add(OBJ.len_utf8()) } else { len };
            out.entry(path.clone()).or_default().push(a.min(len)..b.min(len));
        }
    }
    out
}

/// Cut a paragraph's text into chunks.
fn chunks(p: &wordcraft_doc::Paragraph, skipped: &[Range<usize>]) -> Vec<Chunk> {
    // The formatting at each byte, by run.
    let mut run_at: Vec<(Range<usize>, &CharProps)> = Vec::with_capacity(p.runs.len());
    let mut at = 0usize;
    for r in &p.runs {
        let end = at.saturating_add(r.len);
        run_at.push((at..end, &r.props));
        at = end;
    }
    let props_at = |i: usize| run_at.iter().find(|(r, _)| r.contains(&i)).map_or(&p.mark, |(_, pr)| *pr);
    let mut out = Vec::new();
    let mut cur: Option<Chunk> = None;
    let mut obj = 0usize;
    for (i, c) in p.text.char_indices() {
        let props = props_at(i);
        let end = i + c.len_utf8();
        let marker = c == OBJ && p.objects.get(obj).is_some_and(is_marker);
        if c == OBJ {
            obj += 1;
        }
        let hard = (c == OBJ && !marker)
            || matches!(c, '\t' | LINE_BREAK | PAGE_BREAK | COLUMN_BREAK)
            || props.del.is_some()
            || skipped.iter().any(|r| r.contains(&i));
        if hard {
            out.extend(cur.take());
            continue;
        }
        let ch = cur.get_or_insert_with(|| Chunk { range: i..i, text: String::new(), spans: Vec::new(), markers: Vec::new() });
        ch.range.end = end;
        if marker {
            ch.markers.push((ch.text.len(), props.clone()));
            continue;
        }
        let start = ch.text.len();
        ch.text.push(c);
        match ch.spans.last_mut() {
            Some(s) if s.props == *props && s.range.end == start => s.range.end = ch.text.len(),
            _ => ch.spans.push(Span { range: start..ch.text.len(), props: props.clone() }),
        }
    }
    out.extend(cur);
    out.retain(|c| c.text.chars().any(char::is_alphabetic));
    out
}

impl Plan {
    /// The plan for `scope` of `doc`.
    pub fn new(doc: &Document, scope: &Scope) -> Plan {
        let mut wanted: Vec<(StoryRef, Path)> = match scope {
            Scope::Document => stories(doc).into_iter().flat_map(|s| doc.para_paths(s).into_iter().map(move |p| (s, p))).collect(),
            Scope::Paragraphs(list) => list.clone(),
        };
        wanted.truncate(MAX_PARAS);
        let mut skipped: HashMap<StoryRef, HashMap<Path, Vec<Range<usize>>>> = HashMap::new();
        let mut paras = Vec::new();
        for (story, path) in wanted {
            let Some(p) = doc.para(story, &path) else { continue };
            let skip = skipped.entry(story).or_insert_with(|| citation_spans(doc, story));
            let chunks = chunks(p, skip.get(&path).map(Vec::as_slice).unwrap_or_default());
            if !chunks.is_empty() {
                paras.push(ParaPlan { story, path, chunks });
            }
        }
        Plan { paras }
    }

    /// The texts to translate, each once, in document order (at most [`MAX_TEXTS`]).
    pub fn texts(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for c in self.paras.iter().flat_map(|p| &p.chunks) {
            for t in c.requests() {
                if out.len() >= MAX_TEXTS {
                    return out;
                }
                if seen.insert(t.clone()) {
                    out.push(t);
                }
            }
        }
        out
    }

    /// A sample of the text, to guess its language from.
    pub fn sample(&self, max_chars: usize) -> String {
        let mut out = String::new();
        for c in self.paras.iter().flat_map(|p| &p.chunks) {
            if out.chars().count() >= max_chars {
                break;
            }
            out.push_str(c.text.trim());
            out.push(' ');
        }
        out.chars().take(max_chars).collect()
    }

    /// Whether there is nothing to translate.
    pub fn is_empty(&self) -> bool {
        self.paras.is_empty()
    }

    /// Put the translations into `doc`. A chunk whose text has no translation in the map stays
    /// as it is.
    pub fn apply(&self, doc: &mut Document, translations: &HashMap<String, String>, opts: &ApplyOptions) -> Applied {
        let mut done = Applied::default();
        let to = crate::cmd::local_ai::lang_tag(&opts.to);
        let rtl = wordcraft_ml::lang::is_rtl(&opts.to);
        let complex = is_complex_language(&opts.to);
        for pp in &self.paras {
            let Ok(p) = doc.para_mut(pp.story, &pp.path) else { continue };
            // Text and runs are rebuilt; objects keep their order, so they stay valid.
            let mut pieces: Vec<(String, CharProps)> = Vec::new();
            let mut changed = false;
            let mut at = 0usize;
            for ch in &pp.chunks {
                if ch.range.start < at || ch.range.end > p.text.len() {
                    continue;
                }
                copy_runs(p, at..ch.range.start, &mut pieces);
                match translations.get(ch.text.trim()).map(|t| clean(t)).filter(|t| !t.is_empty()) {
                    Some(t) => {
                        let Mapped { pieces: mapped, markers } = map_chunk(ch, &t, translations);
                        for (k, (text, mut props)) in mapped.into_iter().enumerate() {
                            // Markers keep their own formatting (they carry no text).
                            if !markers.contains(&k) {
                                mark_language(&mut props, &to, rtl, complex);
                            }
                            pieces.push((text, props));
                        }
                        changed = true;
                        done.translated += 1;
                    }
                    None => {
                        copy_runs(p, ch.range.clone(), &mut pieces);
                        done.untranslated += 1;
                    }
                }
                at = ch.range.end;
            }
            if !changed {
                continue;
            }
            copy_runs(p, at..p.text.len(), &mut pieces);
            let mut text = String::new();
            let mut runs: Vec<Run> = Vec::new();
            for (t, props) in pieces {
                if t.is_empty() {
                    continue;
                }
                text.push_str(&t);
                match runs.last_mut() {
                    Some(r) if r.props == props => r.len += t.len(),
                    _ => runs.push(Run { len: t.len(), props }),
                }
            }
            p.text = text;
            p.runs = runs;
            mark_language(&mut p.mark, &to, rtl, complex);
            if rtl {
                p.props.bidi = Some(true);
            } else if p.props.bidi == Some(true) {
                p.props.bidi = Some(false);
            }
            p.touch();
            done.paragraphs += 1;
        }
        done
    }
}

/// Languages whose letters Word draws with the complex-script font and size.
fn is_complex_language(code: &str) -> bool {
    matches!(wordcraft_ml::lang::normalize(code).as_str(), "ar" | "he" | "fa" | "ur" | "yi" | "hi" | "th")
}

/// Mark translated text as written in `to`. Text that moves between Latin-like and complex
/// scripts (Hebrew, Arabic, Thai…) keeps its look: the run's explicit font, size, bold and
/// italic are carried to the complex-script ones it will now be drawn with, and back.
fn mark_language(props: &mut CharProps, to: &str, rtl: bool, complex: bool) {
    if complex {
        if props.font.is_some() && props.font_cs.is_none() {
            props.font_cs.clone_from(&props.font);
        }
        props.size_cs = props.size_cs.or(props.size);
        props.bold_cs = props.bold_cs.or(props.bold);
        props.italic_cs = props.italic_cs.or(props.italic);
        props.lang_bidi = Some(to.to_string());
    } else {
        props.size = props.size.or(props.size_cs);
        props.bold = props.bold.or(props.bold_cs);
        props.italic = props.italic.or(props.italic_cs);
        if props.font.is_none() && props.font_cs.is_some() {
            props.font.clone_from(&props.font_cs);
        }
        props.lang = Some(to.to_string());
    }
    if !rtl && props.rtl == Some(true) {
        props.rtl = None;
    }
}

/// A translation made safe to put in a paragraph: trimmed, without object characters (which
/// would claim objects that aren't there), and with tabs and breaks as spaces (the chunk had none).
fn clean(t: &str) -> String {
    let flat: String = t
        .chars()
        .filter(|c| *c != OBJ)
        .map(|c| if matches!(c, '\t' | '\r' | LINE_BREAK | PAGE_BREAK | COLUMN_BREAK | '\u{000B}' | '\u{2029}') { ' ' } else { c })
        .collect();
    flat.trim().to_string()
}

/// Copy the text and runs of `p` in `range` (bytes) as pieces.
fn copy_runs(p: &wordcraft_doc::Paragraph, range: Range<usize>, out: &mut Vec<(String, CharProps)>) {
    let mut at = 0usize;
    for r in &p.runs {
        let (a, b) = (at.max(range.start), at.saturating_add(r.len).min(range.end));
        if a < b
            && let Some(t) = p.text.get(a..b)
        {
            out.push((t.to_string(), r.props.clone()));
        }
        at = at.saturating_add(r.len);
    }
    // Text past the runs (a paragraph whose runs fall short) takes the paragraph mark's formatting.
    if at < range.end
        && let Some(t) = p.text.get(at.max(range.start)..range.end)
    {
        out.push((t.to_string(), p.mark.clone()));
    }
}

/// A translated chunk: (text, formatting) pieces, markers as single [`OBJ`] pieces, and which
/// piece indexes those are.
struct Mapped {
    pieces: Vec<(String, CharProps)>,
    markers: Vec<usize>,
}

/// Whether position `i` of `chars` is between words (or at an end).
fn at_boundary(chars: &[char], i: usize) -> bool {
    i == 0 || i >= chars.len() || chars.get(i - 1).is_some_and(|c| c.is_whitespace()) || chars.get(i).is_some_and(|c| c.is_whitespace())
}

/// The first case-insensitive occurrence of `needle` in `hay` at or after `from`, as a char range.
fn find_ci(hay: &[char], needle: &[char], from: usize) -> Option<Range<usize>> {
    let low = |c: &char| c.to_lowercase().next().unwrap_or(*c);
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let needle: Vec<char> = needle.iter().map(low).collect();
    (from..=hay.len() - needle.len())
        .find(|&i| hay.get(i..i + needle.len()).is_some_and(|w| w.iter().map(low).eq(needle.iter().copied())))
        .map(|i| i..i + needle.len())
}

/// Map a chunk's formatting and markers onto its translation `t` (the chunk's trimmed text
/// translated).
fn map_chunk(ch: &Chunk, t: &str, translations: &HashMap<String, String>) -> Mapped {
    let src: Vec<char> = ch.text.chars().collect();
    // Byte offset in the chunk text → char index.
    // Byte offset of each char of the chunk's text: byte offset → char index by binary search.
    let starts: Vec<usize> = ch.text.char_indices().map(|(b, _)| b).collect();
    let char_of = |byte: usize| starts.partition_point(|b| *b < byte);
    let (lead, core, _) = wordcraft_ml::text::trim_parts(&ch.text);
    let lead_n = lead.chars().count();
    let core_n = core.chars().count().max(1);
    let tt: Vec<char> = t.chars().collect();
    let spaced = tt.iter().any(|c| c.is_whitespace());
    let trail: Vec<char> = src.get(lead_n + core.chars().count()..).unwrap_or_default().to_vec();
    // The new text: the old leading space, the translation, the old trailing space.
    let mut out_chars: Vec<char> = src.get(..lead_n).unwrap_or_default().to_vec();
    out_chars.extend(&tt);
    out_chars.extend(&trail);
    let span_of_src = |ci: usize| -> usize {
        let byte = starts.get(ci).copied().unwrap_or(ch.text.len());
        ch.spans.iter().position(|s| s.range.contains(&byte)).unwrap_or(0)
    };
    let base = ch.base();
    let mut owner: Vec<usize> = vec![base; out_chars.len()];
    // Leading and trailing space keep the formatting they had.
    for (i, o) in owner.iter_mut().enumerate().take(lead_n) {
        *o = span_of_src(i);
    }
    let trail_start_src = lead_n + core.chars().count();
    for k in 0..trail.len() {
        if let Some(o) = owner.get_mut(lead_n + tt.len() + k) {
            *o = span_of_src(trail_start_src + k);
        }
    }
    // Where a source char index of the core lands in the translation (proportionally).
    let scale = |ci: usize| -> usize {
        let rel = ci.saturating_sub(lead_n).min(core_n) as f64 / core_n as f64;
        ((rel * tt.len() as f64).round() as usize).min(tt.len())
    };
    let base_props = ch.spans.get(base).map(|s| s.props.clone()).unwrap_or_default();
    let mut cursor = 0usize;
    let mut lookups = 0usize;
    for (si, s) in ch.spans.iter().enumerate() {
        if s.props == base_props {
            continue;
        }
        let piece = ch.text.get(s.range.clone()).unwrap_or("").trim();
        let looked_up = piece.chars().any(char::is_alphabetic) && lookups < MAX_SPAN_LOOKUPS;
        lookups += usize::from(looked_up);
        let found = translations
            .get(piece)
            .filter(|_| looked_up)
            .map(|x| clean(x).chars().collect::<Vec<char>>())
            .and_then(|needle| find_ci(&tt, &needle, cursor).or_else(|| find_ci(&tt, &needle, 0)));
        let range = match found {
            Some(r) => {
                cursor = r.end;
                r
            }
            None => {
                // Not found: the words at the same relative place.
                let (a, b) = (char_of(s.range.start), char_of(s.range.end));
                let (mut ta, mut tb) = (scale(a), scale(b));
                if spaced {
                    while ta > 0 && !at_boundary(&tt, ta) {
                        ta -= 1;
                    }
                    while ta < tt.len() && tt.get(ta).is_some_and(|c| c.is_whitespace()) {
                        ta += 1;
                    }
                    while tb < tt.len() && !at_boundary(&tt, tb) {
                        tb += 1;
                    }
                    while tb > ta && tt.get(tb - 1).is_some_and(|c| c.is_whitespace()) {
                        tb -= 1;
                    }
                }
                if !piece.chars().any(char::is_alphabetic) && tb <= ta {
                    continue;
                }
                ta..tb.max(ta)
            }
        };
        for i in range {
            if let Some(o) = owner.get_mut(lead_n + i) {
                *o = si;
            }
        }
    }
    // Markers: their place in the new text, in order.
    let mut marker_at: Vec<(usize, &CharProps)> = Vec::new();
    let mut last = 0usize;
    for (byte, props) in &ch.markers {
        let ci = char_of(*byte);
        let pos = if ci <= lead_n {
            ci
        } else if ci >= trail_start_src {
            lead_n + tt.len() + (ci - trail_start_src)
        } else {
            let raw = scale(ci);
            // After a space in the source: before a word in the translation; before a space:
            // after a word; inside a word: any boundary.
            let ws = |c: Option<&char>| c.is_some_and(|c| c.is_whitespace());
            let (space_before, space_after) = (ws(ci.checked_sub(1).and_then(|i| src.get(i))), ws(src.get(ci)));
            let fits = |i: usize| {
                let before = i == 0 || ws(tt.get(i - 1));
                let after = i >= tt.len() || ws(tt.get(i));
                if space_before {
                    before && (!after || i >= tt.len())
                } else if space_after {
                    after && (!before || i == 0)
                } else {
                    before || after
                }
            };
            let snap = spaced && ch.markers.len() <= MAX_SNAPPED_MARKERS;
            let snapped = if snap { (0..=tt.len()).filter(|i| fits(*i)).min_by_key(|i| i.abs_diff(raw)).unwrap_or(raw) } else { raw };
            lead_n + snapped
        };
        let pos = pos.max(last).min(out_chars.len());
        last = pos;
        marker_at.push((pos, props));
    }
    // Pieces, markers inserted.
    let mut pieces: Vec<(String, CharProps)> = Vec::new();
    let mut markers = Vec::new();
    let mut m = marker_at.iter().peekable();
    for (i, c) in out_chars.iter().enumerate() {
        while let Some((_, props)) = m.next_if(|(p, _)| *p == i) {
            markers.push(pieces.len());
            pieces.push((OBJ.to_string(), (*props).clone()));
        }
        let props = owner.get(i).and_then(|o| ch.spans.get(*o)).map(|s| s.props.clone()).unwrap_or_else(|| base_props.clone());
        let last_is_marker = pieces.len().checked_sub(1).is_some_and(|l| markers.contains(&l));
        match pieces.last_mut() {
            Some((t, p)) if *p == props && !last_is_marker => t.push(*c),
            _ => pieces.push((c.to_string(), props)),
        }
    }
    for (_, props) in m {
        markers.push(pieces.len());
        pieces.push((OBJ.to_string(), (*props).clone()));
    }
    Mapped { pieces, markers }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcraft_doc::para::Paragraph;
    use wordcraft_doc::para_block;

    fn bold() -> CharProps {
        CharProps { bold: Some(true), ..Default::default() }
    }

    fn para(parts: &[(&str, CharProps)]) -> Paragraph {
        let mut p = Paragraph::new();
        for (t, props) in parts {
            p.text.push_str(t);
            p.runs.push(Run { len: t.len(), props: props.clone() });
        }
        p
    }

    fn doc_with(paras: Vec<Paragraph>) -> Document {
        let mut d = Document::new();
        d.body = paras.into_iter().map(para_block).collect();
        d
    }

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn runs_of(p: &Paragraph) -> Vec<(String, bool)> {
        let mut at = 0;
        p.runs
            .iter()
            .map(|r| {
                let t = p.text[at..at + r.len].to_string();
                at += r.len;
                (t, r.props.bold == Some(true))
            })
            .collect()
    }

    #[test]
    fn a_plain_paragraph_is_one_text() {
        let d = doc_with(vec![para(&[("Hello world. ", CharProps::default())]), para(&[("123", CharProps::default())])]);
        let plan = Plan::new(&d, &Scope::Document);
        // Numbers alone aren't translated.
        assert_eq!(plan.texts(), vec!["Hello world."]);
        let mut d2 = d.clone();
        let done = plan.apply(&mut d2, &map(&[("Hello world.", "Hallo Welt.")]), &ApplyOptions { to: "de".into() });
        assert_eq!(done, Applied { paragraphs: 1, translated: 1, untranslated: 0 });
        let p = d2.body[0].as_para().unwrap();
        // The trailing space stays.
        assert_eq!(p.text, "Hallo Welt. ");
        assert_eq!(p.runs[0].props.lang.as_deref(), Some("de-DE"));
        assert_eq!(d2.body[1].as_para().unwrap().text, "123");
    }

    #[test]
    fn a_bold_word_keeps_its_formatting_where_its_translation_is_found() {
        let d = doc_with(vec![para(&[("The ", CharProps::default()), ("red", bold()), (" house is big.", CharProps::default())])]);
        let plan = Plan::new(&d, &Scope::Document);
        assert_eq!(plan.texts(), vec!["The red house is big.", "red"]);
        let mut d2 = d.clone();
        plan.apply(&mut d2, &map(&[("The red house is big.", "La casa roja es grande."), ("red", "Roja")]), &ApplyOptions { to: "es".into() });
        let p = d2.body[0].as_para().unwrap();
        assert_eq!(p.text, "La casa roja es grande.");
        assert_eq!(runs_of(p), vec![("La casa ".into(), false), ("roja".into(), true), (" es grande.".into(), false)]);
    }

    #[test]
    fn formatting_not_found_goes_to_the_words_at_the_same_place() {
        let d = doc_with(vec![para(&[("one two ", CharProps::default()), ("three", bold()), (" four", CharProps::default())])]);
        let plan = Plan::new(&d, &Scope::Document);
        let mut d2 = d.clone();
        plan.apply(&mut d2, &map(&[("one two three four", "un deux trois quatre"), ("three", "XYZ")]), &ApplyOptions { to: "fr".into() });
        let p = d2.body[0].as_para().unwrap();
        assert_eq!(runs_of(p), vec![("un deux ".into(), false), ("trois".into(), true), (" quatre".into(), false)]);
    }

    #[test]
    fn pictures_tabs_and_markers_stay_in_place() {
        let mut p = para(&[("Before\u{FFFC}after\tend \u{FFFC}word here", CharProps::default())]);
        p.objects = vec![
            InlineObject::Field { instr: "PAGE".into(), result: "1".into(), locked: false, dirty: false, code: false, form: String::new() },
            InlineObject::BookmarkStart { name: "b".into() },
        ];
        let d = doc_with(vec![p]);
        let plan = Plan::new(&d, &Scope::Document);
        assert_eq!(plan.texts(), vec!["Before", "after", "end word here"]);
        let mut d2 = d.clone();
        let t = map(&[("Before", "Vorher"), ("after", "danach"), ("end word here", "Ende Wort hier")]);
        let done = plan.apply(&mut d2, &t, &ApplyOptions { to: "de".into() });
        assert_eq!(done.translated, 3);
        let p = d2.body[0].as_para().unwrap();
        // The field stays between the first two texts; the bookmark lands at a word boundary.
        assert_eq!(p.text, "Vorher\u{FFFC}danach\tEnde \u{FFFC}Wort hier");
        assert_eq!(p.objects.len(), 2);
        assert_eq!(p.runs.iter().map(|r| r.len).sum::<usize>(), p.text.len());
    }

    #[test]
    fn missing_translations_leave_text_and_tracked_deletions_stay() {
        let del = CharProps { del: Some(0), ..Default::default() };
        let d = doc_with(vec![para(&[("Keep me", CharProps::default()), (" gone", del.clone()), (" and me", CharProps::default())])]);
        let plan = Plan::new(&d, &Scope::Document);
        assert_eq!(plan.texts(), vec!["Keep me", "and me"]);
        let mut d2 = d.clone();
        let done = plan.apply(&mut d2, &map(&[("Keep me", "Garde-moi")]), &ApplyOptions { to: "fr".into() });
        assert_eq!(done, Applied { paragraphs: 1, translated: 1, untranslated: 1 });
        let p = d2.body[0].as_para().unwrap();
        assert_eq!(p.text, "Garde-moi gone and me");
        assert_eq!(p.runs[1].props.del, Some(0));
    }

    #[test]
    fn every_story_is_planned_and_tables_too() {
        let mut d = wordcraft_doc::Document::from_text("Body text");
        d.parts.insert(
            7,
            wordcraft_doc::Part {
                kind: wordcraft_doc::PartKind::Header,
                blocks: vec![para_block(Paragraph::with_text("Header text", CharProps::default()))],
                body: Default::default(),
            },
        );
        let plan = Plan::new(&d, &Scope::Document);
        assert_eq!(plan.texts(), vec!["Body text", "Header text"]);
        let only = Plan::new(&d, &Scope::Paragraphs(vec![(StoryRef::Part(7), Path::top(0))]));
        assert_eq!(only.texts(), vec!["Header text"]);
        // A path that doesn't exist is skipped.
        assert!(Plan::new(&d, &Scope::Paragraphs(vec![(StoryRef::Part(99), Path::top(0))])).is_empty());
    }

    #[test]
    fn into_hebrew_paragraphs_turn_right_to_left_and_keep_their_size() {
        let big = CharProps { size: Some(16.0), bold: Some(true), font: Some("Arial".into()), ..Default::default() };
        let d = doc_with(vec![para(&[("Hello", big)])]);
        let plan = Plan::new(&d, &Scope::Document);
        let mut d2 = d.clone();
        plan.apply(&mut d2, &map(&[("Hello", "שלום")]), &ApplyOptions { to: "he".into() });
        let p = d2.body[0].as_para().unwrap();
        assert_eq!(p.text, "שלום");
        assert_eq!(p.props.bidi, Some(true));
        let r = &p.runs[0].props;
        assert_eq!((r.size_cs, r.bold_cs, r.font_cs.as_deref(), r.lang_bidi.as_deref()), (Some(16.0), Some(true), Some("Arial"), Some("he-IL")));
        // And back: a right-to-left paragraph translated into English reads left to right.
        let plan = Plan::new(&d2, &Scope::Document);
        let mut d3 = d2.clone();
        plan.apply(&mut d3, &map(&[("שלום", "Hello")]), &ApplyOptions { to: "en".into() });
        assert_eq!(d3.body[0].as_para().unwrap().props.bidi, Some(false));
    }

    #[test]
    fn thousands_of_runs_stay_bounded() {
        // Every word in its own formatting: one text for the paragraph, a bounded number of lookups.
        let parts: Vec<(String, CharProps)> = (0..5000)
            .map(|i| (format!("w{i} "), CharProps { size: Some(8.0 + (i % 50) as f32), bold: Some(i % 2 == 0), ..Default::default() }))
            .collect();
        let refs: Vec<(&str, CharProps)> = parts.iter().map(|(t, p)| (t.as_str(), p.clone())).collect();
        let mut p = para(&refs);
        p.text.push('\u{FFFC}');
        p.runs.push(Run { len: 3, props: CharProps::default() });
        p.objects = vec![InlineObject::BookmarkStart { name: "b".into() }];
        let d = doc_with(vec![p]);
        let started = std::time::Instant::now();
        let plan = Plan::new(&d, &Scope::Document);
        let texts = plan.texts();
        assert!(texts.len() <= 1 + MAX_SPAN_LOOKUPS, "{}", texts.len());
        let m: HashMap<String, String> = texts.iter().map(|t| (t.clone(), t.to_uppercase())).collect();
        let mut d2 = d.clone();
        assert_eq!(plan.apply(&mut d2, &m, &ApplyOptions { to: "de".into() }).translated, 1);
        let p = d2.body[0].as_para().unwrap();
        assert_eq!(p.runs.iter().map(|r| r.len).sum::<usize>(), p.text.len());
        assert_eq!(p.text.matches('\u{FFFC}').count(), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    }

    #[test]
    fn hostile_mappings_never_panic() {
        // Translations far shorter or longer than the source, empty, or with odd characters.
        let d = doc_with(vec![para(&[("a ", bold()), ("b c d e f", CharProps::default()), (" g", bold())])]);
        let plan = Plan::new(&d, &Scope::Document);
        for t in ["x", "", " ", "日本語のテキスト", "a\u{FFFC}b", "word word word word word word word word word word"] {
            let mut d2 = d.clone();
            let mut m = HashMap::new();
            for k in plan.texts() {
                m.insert(k, t.to_string());
            }
            plan.apply(&mut d2, &m, &ApplyOptions { to: "ja".into() });
            let p = d2.body[0].as_para().unwrap();
            assert_eq!(p.runs.iter().map(|r| r.len).sum::<usize>(), p.text.len(), "{t:?}");
        }
    }
}
