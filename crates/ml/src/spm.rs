//! SentencePiece unigram models (`source.spm`, `target.spm`), read from their protocol-buffer
//! file and used to cut text into the pieces a Marian translation model was trained on.
//!
//! The file is the `ModelProto` message of the SentencePiece project (Apache-2.0); only what
//! encoding needs is read: the pieces with their scores and kinds, and three normaliser switches.
//! Text is cut by the most probable segmentation (Viterbi over the piece scores), as SentencePiece
//! does. Its compiled NFKC normalisation table isn't applied: text reaches the model as typed,
//! which changes nothing for ordinary text (compatibility characters such as full-width Latin
//! letters may come out as unknown pieces).

use std::collections::HashMap;

use crate::{Error, Result};

/// The word-boundary mark SentencePiece puts in place of spaces.
pub const SPACE: char = '\u{2581}';
/// Most pieces a model may have.
const MAX_PIECES: usize = 1 << 20;
/// Longest piece considered, in characters.
const MAX_PIECE_CHARS: usize = 64;
/// Longest text cut in one go, in characters (longer text is cut in parts by the caller).
pub const MAX_TEXT_CHARS: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Normal,
    Unknown,
    Control,
    UserDefined,
    Unused,
    Byte,
}

/// A SentencePiece model.
#[derive(Clone, Debug)]
pub struct Model {
    pieces: HashMap<String, (f32, Kind)>,
    longest: usize,
    unknown_score: f32,
    add_dummy_prefix: bool,
    remove_extra_whitespaces: bool,
}

/// A protocol-buffer reader over a byte slice that never reads past its end.
struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.at)?;
            self.at += 1;
            v |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.varint()?).ok()?;
        let end = self.at.checked_add(len)?;
        let b = self.buf.get(self.at..end)?;
        self.at = end;
        Some(b)
    }

    fn fixed32(&mut self) -> Option<[u8; 4]> {
        let b: [u8; 4] = self.buf.get(self.at..self.at.checked_add(4)?)?.try_into().ok()?;
        self.at += 4;
        Some(b)
    }

    /// The next field: (number, wire type), or `None` at the end.
    fn key(&mut self) -> Option<(u64, u8)> {
        if self.at >= self.buf.len() {
            return None;
        }
        let k = self.varint()?;
        Some((k >> 3, (k & 7) as u8))
    }

    /// Skip a field's value; `None` if the data is broken.
    fn skip(&mut self, wire: u8) -> Option<()> {
        match wire {
            0 => self.varint().map(|_| ()),
            1 => {
                self.at = self.at.checked_add(8).filter(|e| *e <= self.buf.len())?;
                Some(())
            }
            2 => self.bytes().map(|_| ()),
            5 => self.fixed32().map(|_| ()),
            _ => None,
        }
    }
}

impl Model {
    /// Read a `.spm` file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let bad = || Error::Model("the SentencePiece model (.spm) is damaged".into());
        let mut r = Reader { buf: bytes, at: 0 };
        let mut pieces = HashMap::new();
        let mut longest = 1;
        let mut min_score = 0f32;
        let (mut add_dummy_prefix, mut remove_extra_whitespaces) = (true, true);
        let mut model_type = 1u64;
        while let Some((field, wire)) = r.key() {
            match (field, wire) {
                (1, 2) => {
                    let mut p = Reader { buf: r.bytes().ok_or_else(bad)?, at: 0 };
                    let (mut text, mut score, mut kind) = (String::new(), 0f32, Kind::Normal);
                    while let Some((f, w)) = p.key() {
                        match (f, w) {
                            (1, 2) => text = String::from_utf8_lossy(p.bytes().ok_or_else(bad)?).into_owned(),
                            (2, 5) => score = f32::from_le_bytes(p.fixed32().ok_or_else(bad)?),
                            (3, 0) => {
                                kind = match p.varint().ok_or_else(bad)? {
                                    2 => Kind::Unknown,
                                    3 => Kind::Control,
                                    4 => Kind::UserDefined,
                                    5 => Kind::Unused,
                                    6 => Kind::Byte,
                                    _ => Kind::Normal,
                                }
                            }
                            _ => p.skip(w).ok_or_else(bad)?,
                        }
                    }
                    if pieces.len() >= MAX_PIECES {
                        return Err(Error::Model("the SentencePiece model has too many pieces".into()));
                    }
                    if !score.is_finite() {
                        score = 0.0;
                    }
                    if kind == Kind::Normal {
                        min_score = min_score.min(score);
                    }
                    longest = longest.max(text.chars().count().min(MAX_PIECE_CHARS));
                    pieces.entry(text).or_insert((score, kind));
                }
                (2, 2) => {
                    // TrainerSpec: only the model type matters (1 = unigram).
                    let mut t = Reader { buf: r.bytes().ok_or_else(bad)?, at: 0 };
                    while let Some((f, w)) = t.key() {
                        if (f, w) == (3, 0) {
                            model_type = t.varint().ok_or_else(bad)?;
                        } else {
                            t.skip(w).ok_or_else(bad)?;
                        }
                    }
                }
                (3, 2) => {
                    let mut n = Reader { buf: r.bytes().ok_or_else(bad)?, at: 0 };
                    while let Some((f, w)) = n.key() {
                        match (f, w) {
                            (3, 0) => add_dummy_prefix = n.varint().ok_or_else(bad)? != 0,
                            (4, 0) => remove_extra_whitespaces = n.varint().ok_or_else(bad)? != 0,
                            _ => n.skip(w).ok_or_else(bad)?,
                        }
                    }
                }
                _ => r.skip(wire).ok_or_else(bad)?,
            }
        }
        if pieces.is_empty() {
            return Err(Error::Model("the SentencePiece model has no pieces".into()));
        }
        if model_type != 1 {
            return Err(Error::Model("only unigram SentencePiece models are supported".into()));
        }
        Ok(Model { pieces, longest, unknown_score: min_score - 10.0, add_dummy_prefix, remove_extra_whitespaces })
    }

    /// The text as the model sees it: spaces as [`SPACE`], extra spaces removed, a leading mark.
    fn normalize(&self, text: &str) -> String {
        let text: String = text.chars().take(MAX_TEXT_CHARS).map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
        let text = if self.remove_extra_whitespaces { text.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ") } else { text };
        let mut out = String::with_capacity(text.len() + 3);
        if self.add_dummy_prefix && !text.is_empty() {
            out.push(SPACE);
        }
        out.extend(text.chars().map(|c| if c == ' ' { SPACE } else { c }));
        out
    }

    /// Cut text into pieces. Characters the model has no piece for come out as their own text
    /// (a run of them as one piece), which a vocabulary maps to its unknown token.
    pub fn encode(&self, text: &str) -> Vec<String> {
        let norm = self.normalize(text);
        // Byte offset of each character, and of the end.
        let offsets: Vec<usize> = norm.char_indices().map(|(i, _)| i).chain(std::iter::once(norm.len())).collect();
        let n = offsets.len() - 1;
        // best[i]: (score, start of the last piece, that piece unknown) of the best cut of the first i chars.
        let mut best: Vec<Option<(f32, usize, bool)>> = vec![None; n + 1];
        if let Some(b) = best.get_mut(0) {
            *b = Some((0.0, 0, false));
        }
        for i in 0..n {
            let Some(Some((score, _, _))) = best.get(i).copied() else { continue };
            let mut matched_single = false;
            for len in 1..=self.longest.min(n - i) {
                let (Some(&a), Some(&b)) = (offsets.get(i), offsets.get(i + len)) else { break };
                let Some(piece) = norm.get(a..b) else { break };
                let Some((s, kind)) = self.pieces.get(piece) else { continue };
                if !matches!(kind, Kind::Normal | Kind::UserDefined) {
                    continue;
                }
                if len == 1 {
                    matched_single = true;
                }
                // User-defined pieces always win their span.
                let s = if *kind == Kind::UserDefined { 0.0 } else { *s };
                let cand = score + s;
                if let Some(slot) = best.get_mut(i + len)
                    && slot.is_none_or(|(old, _, _)| cand > old)
                {
                    *slot = Some((cand, i, false));
                }
            }
            if !matched_single {
                let cand = score + self.unknown_score;
                if let Some(slot) = best.get_mut(i + 1)
                    && slot.is_none_or(|(old, _, _)| cand > old)
                {
                    *slot = Some((cand, i, true));
                }
            }
        }
        // Walk back from the end.
        let mut cuts: Vec<(usize, usize, bool)> = Vec::new();
        let mut end = n;
        while end > 0 {
            let Some(Some((_, start, unknown))) = best.get(end).copied() else { break };
            cuts.push((start, end, unknown));
            end = start;
        }
        cuts.reverse();
        let mut out: Vec<String> = Vec::with_capacity(cuts.len());
        let mut last_unknown = false;
        for (a, b, unknown) in cuts {
            let (Some(&a), Some(&b)) = (offsets.get(a), offsets.get(b)) else { continue };
            let piece = norm.get(a..b).unwrap_or_default();
            match out.last_mut() {
                // Unknown characters in a row make one unknown piece, as in SentencePiece.
                Some(prev) if unknown && last_unknown => prev.push_str(piece),
                _ => out.push(piece.to_string()),
            }
            last_unknown = unknown;
        }
        out
    }

    /// Whether the model has this piece.
    pub fn contains(&self, piece: &str) -> bool {
        self.pieces.contains_key(piece)
    }
}

/// Pieces back to text: joined, with [`SPACE`] as a space and the leading one dropped.
pub fn decode(pieces: &[&str]) -> String {
    let joined: String = pieces.concat();
    let text: String = joined.chars().map(|c| if c == SPACE { ' ' } else { c }).collect();
    text.strip_prefix(' ').map(str::to_string).unwrap_or(text)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn field_bytes(field: u64, b: &[u8], out: &mut Vec<u8>) {
        varint(field << 3 | 2, out);
        varint(b.len() as u64, out);
        out.extend_from_slice(b);
    }

    /// A `.spm` file holding these (piece, score) pairs, plus `<unk>`, `<s>` and `</s>`.
    pub(crate) fn build(pieces: &[(&str, f32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut all: Vec<(&str, f32, u64)> = vec![("<unk>", 0.0, 2), ("<s>", 0.0, 3), ("</s>", 0.0, 3)];
        all.extend(pieces.iter().map(|(p, s)| (*p, *s, 1)));
        for (p, s, kind) in all {
            let mut m = Vec::new();
            field_bytes(1, p.as_bytes(), &mut m);
            varint(2 << 3 | 5, &mut m);
            m.extend_from_slice(&s.to_le_bytes());
            varint(3 << 3, &mut m);
            varint(kind, &mut m);
            field_bytes(1, &m, &mut out);
        }
        // TrainerSpec: unigram; NormalizerSpec: defaults.
        let mut t = Vec::new();
        varint(3 << 3, &mut t);
        varint(1, &mut t);
        field_bytes(2, &t, &mut out);
        let mut n = Vec::new();
        field_bytes(1, b"nmt_nfkc", &mut n);
        field_bytes(3, &n, &mut out);
        out
    }

    pub(crate) fn english() -> Model {
        Model::parse(&build(&[
            ("▁the", -2.0),
            ("▁cat", -3.0),
            ("▁c", -5.0),
            ("at", -4.0),
            ("▁", -1.0),
            ("s", -3.0),
            ("a", -6.0),
            ("t", -6.0),
            ("c", -6.0),
            ("h", -6.0),
            ("e", -6.0),
            (".", -2.0),
        ]))
        .unwrap()
    }

    #[test]
    fn cuts_text_into_the_likeliest_pieces() {
        let m = english();
        assert_eq!(m.encode("the cat"), vec!["▁the", "▁cat"]);
        assert_eq!(m.encode("  the   cats. "), vec!["▁the", "▁cat", "s", "."]);
        // Characters without a piece become one unknown piece per run.
        assert_eq!(m.encode("the €€ cat"), vec!["▁the", "▁", "€€", "▁cat"]);
        assert!(m.encode("").is_empty());
        assert!(m.encode("   ").is_empty());
    }

    #[test]
    fn decoding_restores_spaces() {
        assert_eq!(decode(&["▁the", "▁cat", "s", "."]), "the cats.");
        assert_eq!(decode(&["▁שלום", "▁עולם"]), "שלום עולם");
        assert_eq!(decode(&[]), "");
    }

    #[test]
    fn damaged_files_are_errors_not_crashes() {
        let good = build(&[("▁a", -1.0)]);
        assert!(Model::parse(&good).is_ok());
        assert!(Model::parse(&[]).is_err());
        for cut in 1..good.len() {
            let _ = Model::parse(&good[..cut]);
        }
        assert!(Model::parse(&[0x0A, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).is_err());
        assert!(Model::parse(&[0x0F]).is_err());
    }

    proptest::proptest! {
        #[test]
        fn any_text_encodes_and_the_pieces_spell_it(text in "[\\PC&&[^\u{2581}]]{0,40}") {
            let m = english();
            let pieces = m.encode(&text);
            let refs: Vec<&str> = pieces.iter().map(String::as_str).collect();
            let expected = text.split_whitespace().collect::<Vec<_>>().join(" ");
            proptest::prop_assert_eq!(decode(&refs), expected);
        }

        #[test]
        fn any_bytes_parse_without_panicking(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..200)) {
            let _ = Model::parse(&bytes);
        }
    }
}
