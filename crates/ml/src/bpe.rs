//! Whisper's tokens: the byte-level BPE vocabulary of a Hugging Face `tokenizer.json`. Dictation
//! only turns token ids back into text and looks up the special tokens (start of transcript,
//! languages, end of text), so no merges are needed.

use std::collections::HashMap;

use serde_json::Value;

use crate::{Error, Result};

/// Highest token id accepted from a file (Whisper's vocabularies have about 52,000).
const MAX_ID: u64 = 1 << 20;

/// GPT-2's byte-to-character table: every byte gets a printable character, so tokens are
/// strings. Printable Latin-1 bytes stand for themselves; the rest map to U+0100 onwards.
fn byte_chars() -> [char; 256] {
    let mut table = ['\0'; 256];
    let mut extra = 0u32;
    for (b, slot) in table.iter_mut().enumerate() {
        let printable = (0x21..=0x7E).contains(&b) || (0xA1..=0xAC).contains(&b) || (0xAE..=0xFF).contains(&b);
        let code = if printable {
            b as u32
        } else {
            extra += 1;
            255 + extra
        };
        *slot = char::from_u32(code).unwrap_or('\u{FFFD}');
    }
    table
}

/// A Whisper vocabulary.
#[derive(Clone, Debug)]
pub struct Vocab {
    /// Token text by id (empty for ids the file doesn't use).
    tokens: Vec<String>,
    /// Ids of the added (special) tokens: `<|endoftext|>`, `<|en|>`…
    special: HashMap<String, u32>,
    /// Character back to byte.
    bytes: HashMap<char, u8>,
}

impl Vocab {
    /// Read a Hugging Face `tokenizer.json`.
    pub fn from_tokenizer_json(text: &str) -> Result<Self> {
        let bad = |why: &str| Error::Model(format!("tokenizer.json: {why}"));
        let v: Value = serde_json::from_str(text).map_err(|e| bad(&e.to_string()))?;
        let vocab = v.pointer("/model/vocab").and_then(Value::as_object).ok_or_else(|| bad("no model.vocab"))?;
        let mut pairs: Vec<(String, u32)> = Vec::with_capacity(vocab.len());
        for (token, id) in vocab {
            let id = id.as_u64().filter(|i| *i < MAX_ID).ok_or_else(|| bad("a token id is out of range"))?;
            pairs.push((token.clone(), id as u32));
        }
        let mut special = HashMap::new();
        for t in v.get("added_tokens").and_then(Value::as_array).into_iter().flatten() {
            let (Some(id), Some(content)) = (t.get("id").and_then(Value::as_u64), t.get("content").and_then(Value::as_str)) else { continue };
            if id < MAX_ID {
                special.insert(content.to_string(), id as u32);
                pairs.push((content.to_string(), id as u32));
            }
        }
        Self::build(pairs, special)
    }

    fn build(pairs: Vec<(String, u32)>, special: HashMap<String, u32>) -> Result<Self> {
        let len = pairs.iter().map(|(_, id)| *id as usize + 1).max().unwrap_or(0);
        if len == 0 {
            return Err(Error::Model("tokenizer.json has no tokens".into()));
        }
        let mut tokens = vec![String::new(); len];
        for (t, id) in pairs {
            if let Some(slot) = tokens.get_mut(id as usize) {
                *slot = t;
            }
        }
        let bytes = byte_chars().iter().enumerate().map(|(b, c)| (*c, b as u8)).collect();
        Ok(Vocab { tokens, special, bytes })
    }

    /// A special token's id (`<|startoftranscript|>`, `<|he|>`…).
    pub fn special(&self, token: &str) -> Option<u32> {
        self.special.get(token).copied()
    }

    /// Whether this id is a special token (never part of the text).
    pub fn is_special(&self, id: u32) -> bool {
        self.tokens.get(id as usize).is_some_and(|t| self.special.get(t) == Some(&id))
    }

    /// The language tokens the model knows, as (code, id), in id order: `("en", 50259)`…
    pub fn languages(&self) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = self
            .special
            .iter()
            .filter_map(|(t, id)| {
                let code = t.strip_prefix("<|")?.strip_suffix("|>")?;
                let is_lang = (2..=3).contains(&code.len()) && code.chars().all(|c| c.is_ascii_lowercase()) || code == "haw" || code == "yue";
                is_lang.then(|| (code.to_string(), *id))
            })
            .collect();
        out.sort_by_key(|(_, id)| *id);
        out
    }

    /// The text of these tokens, leaving out special ones.
    pub fn decode(&self, ids: &[u32]) -> String {
        let mut bytes = Vec::new();
        for id in ids {
            if self.is_special(*id) {
                continue;
            }
            let Some(t) = self.tokens.get(*id as usize) else { continue };
            for c in t.chars() {
                match self.bytes.get(&c) {
                    Some(b) => bytes.push(*b),
                    // A character outside the byte table: keep it as it is.
                    None => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
                }
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// How many token ids the vocabulary spans.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny tokenizer.json in Whisper's shape.
    pub(crate) fn sample() -> String {
        let table = byte_chars();
        let enc = |s: &str| s.bytes().map(|b| table[b as usize]).collect::<String>();
        serde_json::json!({
            "model": {"type": "BPE", "vocab": {
                enc("Hello"): 0, enc(" world"): 1, enc("!"): 2, enc(" שלום"): 3,
                // The first byte of "é" alone, then the second: a character split over two tokens.
                table[0xC3].to_string(): 4, table[0xA9].to_string(): 5,
            }},
            "added_tokens": [
                {"id": 6, "content": "<|endoftext|>"},
                {"id": 7, "content": "<|startoftranscript|>"},
                {"id": 8, "content": "<|en|>"},
                {"id": 9, "content": "<|he|>"},
                {"id": 10, "content": "<|transcribe|>"},
                {"id": 11, "content": "<|notimestamps|>"},
            ]
        })
        .to_string()
    }

    #[test]
    fn decodes_bytes_back_to_text() {
        let v = Vocab::from_tokenizer_json(&sample()).unwrap();
        assert_eq!(v.decode(&[7, 8, 0, 1, 2, 6]), "Hello world!");
        assert_eq!(v.decode(&[3]), " שלום");
        assert_eq!(v.decode(&[4, 5]), "é");
        // A lone half of a character, and ids past the end, don't break anything.
        assert_eq!(v.decode(&[4, 999_999]), "\u{FFFD}");
        assert_eq!(v.special("<|he|>"), Some(9));
        assert!(v.is_special(6) && !v.is_special(0));
        assert_eq!(v.languages(), vec![("en".to_string(), 8), ("he".to_string(), 9)]);
    }

    #[test]
    fn the_byte_table_is_a_bijection() {
        let t = byte_chars();
        let set: std::collections::HashSet<char> = t.iter().copied().collect();
        assert_eq!(set.len(), 256);
        assert_eq!(t[b'A' as usize], 'A');
        assert_eq!(t[b' ' as usize], 'Ġ');
    }

    #[test]
    fn bad_files_are_errors() {
        assert!(Vocab::from_tokenizer_json("").is_err());
        assert!(Vocab::from_tokenizer_json("{}").is_err());
        assert!(Vocab::from_tokenizer_json(r#"{"model":{"vocab":{"a":-1}}}"#).is_err());
        assert!(Vocab::from_tokenizer_json(r#"{"model":{"vocab":{"a":99999999999}}}"#).is_err());
        assert!(Vocab::from_tokenizer_json(r#"{"model":{"vocab":{}}}"#).is_err());
    }
}
