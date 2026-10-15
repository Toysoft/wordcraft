//! Translate Selection without a Google key: looks each selected word up in StarDict dictionaries
//! (`.ifo` + `.idx` + `.dict` or `.dict.dz`) the user puts in the dictionaries folder (File ›
//! Options › Proofing) or the other folders spelling dictionaries are searched in. Nothing is
//! bundled and nothing leaves the machine. Files are untrusted: sizes are capped, offsets checked.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The largest `.idx` or (inflated) `.dict` read; common bilingual dictionaries are far smaller.
const MAX_BYTES: usize = 256 << 20;
/// The most dictionaries loaded, words looked up, and characters shown per entry.
const MAX_DICTS: usize = 8;
pub const MAX_WORDS: usize = 50;
const MAX_ENTRY_CHARS: usize = 2000;

pub struct Dict {
    /// Lower-cased headword, entry offset and size in `data`, sorted by headword.
    entries: Vec<(String, usize, usize)>,
    data: Vec<u8>,
    /// `sametypesequence`: the entries' field types, when they carry no type markers of their own.
    types: Option<String>,
}

/// Dictionaries loaded, by the `.ifo` files they came from.
/// shortcut: a file replaced under the same name is not reloaded until restart; key on mtime if that matters.
static LOADED: Mutex<Vec<(PathBuf, Arc<Dict>)>> = Mutex::new(Vec::new());

/// Each word of `text` (or the whole text, when it is a headword) with its entry, or `None` when
/// no dictionary is installed.
pub fn look_up(text: &str) -> Option<Vec<(String, Option<String>)>> {
    let dicts = installed();
    if dicts.is_empty() {
        return None;
    }
    let find = |w: &str| dicts.iter().find_map(|d| d.lookup(w));
    let phrase = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if phrase.contains(' ')
        && let Some(e) = find(&phrase)
    {
        return Some(vec![(phrase, Some(e))]);
    }
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for w in words(text) {
        if out.len() >= MAX_WORDS {
            break;
        }
        if !out.iter().any(|(seen, _)| seen.to_lowercase() == w.to_lowercase()) {
            out.push((w.to_string(), find(w)));
        }
    }
    Some(out)
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’' || c == '-'))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
}

fn installed() -> Vec<Arc<Dict>> {
    let mut ifos: Vec<PathBuf> = Vec::new();
    for dir in wordcraft_engine::proof::hunspell::search_dirs(None) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut here: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "ifo")).collect();
        here.sort();
        ifos.extend(here);
    }
    ifos.truncate(MAX_DICTS);
    let Ok(mut loaded) = LOADED.lock() else { return Vec::new() };
    loaded.retain(|(p, _)| ifos.contains(p));
    let mut out = Vec::new();
    for ifo in ifos {
        if let Some((_, d)) = loaded.iter().find(|(p, _)| *p == ifo) {
            out.push(d.clone());
        } else {
            match Dict::open(&ifo) {
                Ok(d) => {
                    let d = Arc::new(d);
                    loaded.push((ifo, d.clone()));
                    out.push(d);
                }
                Err(e) => log::warn!("{}: {e}", ifo.display()),
            }
        }
    }
    out
}

fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_BYTES as u64 {
        return Err("too large".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn be(b: &[u8]) -> usize {
    b.iter().fold(0usize, |n, &x| n.saturating_mul(256).saturating_add(x as usize))
}

/// The deflate stream inside a gzip (dictzip) file, inflated.
fn gunzip(gz: &[u8]) -> Result<Vec<u8>, String> {
    let bad = || "not a gzip file".to_string();
    let flags = match gz {
        [0x1f, 0x8b, 8, flags, ..] => *flags,
        _ => return Err(bad()),
    };
    let mut at = 10usize;
    if flags & 4 != 0 {
        let xlen = gz.get(at..at + 2).map(|b| usize::from(b[0]) | usize::from(b[1]) << 8).ok_or_else(bad)?;
        at = at.saturating_add(2).saturating_add(xlen);
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            at = at.saturating_add(gz.get(at..).and_then(|r| r.iter().position(|&b| b == 0)).ok_or_else(bad)? + 1);
        }
    }
    if flags & 2 != 0 {
        at = at.saturating_add(2);
    }
    let body = gz.get(at..).ok_or_else(bad)?;
    miniz_oxide::inflate::decompress_to_vec_with_limit(body, MAX_BYTES).map_err(|e| format!("inflate failed: {e:?}"))
}

impl Dict {
    fn open(ifo: &Path) -> Result<Dict, String> {
        let info = String::from_utf8_lossy(&read_capped(ifo)?).into_owned();
        let field = |k: &str| info.lines().find_map(|l| l.trim().strip_prefix(k)?.strip_prefix('=').map(str::trim));
        let wide = field("idxoffsetbits") == Some("64");
        let types = field("sametypesequence").filter(|t| !t.is_empty()).map(str::to_string);
        let idx = read_capped(&ifo.with_extension("idx"))?;
        let data = match read_capped(&ifo.with_extension("dict")) {
            Ok(d) => d,
            Err(_) => gunzip(&read_capped(&ifo.with_extension("dict.dz"))?)?,
        };
        Ok(Dict { entries: entries(&idx, wide), data, types })
    }

    fn lookup(&self, word: &str) -> Option<String> {
        let w = word.to_lowercase().replace('’', "'");
        // shortcut: English inflections only; other languages need their Hunspell affix rules.
        let stems = [
            Some(w.clone()),
            w.strip_suffix("'s").map(str::to_string),
            w.strip_suffix("ies").map(|s| format!("{s}y")),
            w.strip_suffix("es").map(str::to_string),
            w.strip_suffix('s').map(str::to_string),
            w.strip_suffix("ied").map(|s| format!("{s}y")),
            w.strip_suffix("ed").map(str::to_string),
            w.strip_suffix('d').map(str::to_string),
            w.strip_suffix("ing").map(|s| format!("{s}e")),
            w.strip_suffix("ing").map(str::to_string),
            w.strip_suffix("ly").map(str::to_string),
        ];
        stems.into_iter().flatten().filter(|s| !s.is_empty()).find_map(|s| self.entry(&s))
    }

    fn entry(&self, key: &str) -> Option<String> {
        let from = self.entries.partition_point(|(k, ..)| k.as_str() < key);
        let text: Vec<String> = self
            .entries
            .get(from..)?
            .iter()
            .take_while(|(k, ..)| k == key)
            .filter_map(|&(_, off, size)| self.data.get(off..off.checked_add(size)?))
            .map(|chunk| fields(chunk, self.types.as_deref()))
            .filter(|t| !t.is_empty())
            .collect();
        let text = text.join("\n\n");
        (!text.is_empty()).then(|| match text.char_indices().nth(MAX_ENTRY_CHARS) {
            Some((cut, _)) => format!("{}…", &text[..cut]),
            None => text,
        })
    }
}

/// The `.idx` entries: a NUL-terminated headword, then a big-endian offset (32 or 64 bits) and size.
fn entries(idx: &[u8], wide: bool) -> Vec<(String, usize, usize)> {
    let width = if wide { 8 } else { 4 };
    let mut out = Vec::new();
    let mut rest = idx;
    while let Some(nul) = rest.iter().position(|&b| b == 0) {
        let Some((word, tail)) = rest.split_at_checked(nul) else { break };
        let Some(nums) = tail.get(1..1 + width + 4) else { break };
        out.push((String::from_utf8_lossy(word).to_lowercase(), be(&nums[..width]), be(&nums[width..])));
        rest = tail.get(1 + width + 4..).unwrap_or_default();
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The readable text of one entry. Lower-case field types are text (NUL-terminated), upper-case
/// ones binary (size-prefixed); the last field of a `sametypesequence` entry runs to its end.
fn fields(chunk: &[u8], types: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut rest = chunk;
    let mut seq = types.map(|t| t.bytes());
    let mut left = types.map_or(0, str::len);
    while !rest.is_empty() {
        let t = match seq.as_mut() {
            Some(s) => match s.next() {
                Some(t) => t,
                None => break,
            },
            None => match rest.split_first() {
                Some((&t, r)) => {
                    rest = r;
                    t
                }
                None => break,
            },
        };
        left = left.saturating_sub(1);
        let last = seq.is_some() && left == 0;
        let body: &[u8] = if last {
            std::mem::take(&mut rest)
        } else if t.is_ascii_lowercase() {
            let n = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            let (body, tail) = rest.split_at(n);
            rest = tail.get(1..).unwrap_or_default();
            body
        } else {
            let n = rest.get(..4).map_or(0, be).min(rest.len().saturating_sub(4));
            let body = rest.get(4..4 + n).unwrap_or_default();
            rest = rest.get(4 + n..).unwrap_or_default();
            body
        };
        if t.is_ascii_lowercase() {
            out.push(plain(&String::from_utf8_lossy(body)));
        }
    }
    out.retain(|s| !s.is_empty());
    out.join("\n")
}

/// Markup (HTML, Pango, XDXF) as plain text: tags dropped, line-breaking ones kept as new lines.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            rest = &rest[open..];
            break;
        };
        let tag = rest[open + 1..open + close].trim_start_matches('/').to_ascii_lowercase();
        if ["br", "p", "div", "li", "tr"].iter().any(|t| tag == *t || tag.starts_with(&format!("{t} "))) {
            out.push('\n');
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    let out = out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&nbsp;", " ").replace("&amp;", "&");
    out.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A StarDict dictionary of `(headword, definition)` pairs, with the `.dict` compressed when `dz`.
    fn write(dir: &Path, name: &str, words: &[(&str, &str)], types: Option<&str>, dz: bool) {
        let (mut idx, mut dict) = (Vec::new(), Vec::new());
        for (w, d) in words {
            idx.extend_from_slice(w.as_bytes());
            idx.push(0);
            idx.extend_from_slice(&(dict.len() as u32).to_be_bytes());
            idx.extend_from_slice(&(d.len() as u32).to_be_bytes());
            dict.extend_from_slice(d.as_bytes());
        }
        let seq = types.map_or(String::new(), |t| format!("sametypesequence={t}\n"));
        let ifo = format!("StarDict's dict ifo file\nversion=2.4.2\nwordcount={}\nbookname={name}\n{seq}", words.len());
        std::fs::write(dir.join(format!("{name}.ifo")), ifo).unwrap();
        std::fs::write(dir.join(format!("{name}.idx")), idx).unwrap();
        if dz {
            let mut gz = vec![0x1f, 0x8b, 8, 8 | 4, 0, 0, 0, 0, 0, 3, 2, 0, b'R', b'A'];
            gz.extend_from_slice(b"name.dict\0");
            gz.extend(miniz_oxide::deflate::compress_to_vec(&dict, 6));
            std::fs::write(dir.join(format!("{name}.dict.dz")), gz).unwrap();
        } else {
            std::fs::write(dir.join(format!("{name}.dict")), dict).unwrap();
        }
    }

    #[test]
    fn reads_plain_and_dictzip_dictionaries() {
        let dir = std::env::temp_dir().join(format!("wordcraft-stardict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "plain", &[("Hello", "xin chào"), ("world", "<b>thế giới</b><br>trái đất")], Some("h"), false);
        write(&dir, "zipped", &[("cat", "mcon mèo\0"), ("look up", "mtra cứu\0")], None, true);
        let d = Dict::open(&dir.join("plain.ifo")).unwrap();
        assert_eq!(d.lookup("hello").as_deref(), Some("xin chào"));
        assert_eq!(d.lookup("Worlds").as_deref(), Some("thế giới\ntrái đất"));
        assert_eq!(d.lookup("moon"), None);
        let z = Dict::open(&dir.join("zipped.ifo")).unwrap();
        assert_eq!(z.lookup("cats").as_deref(), Some("con mèo"));
        assert_eq!(z.entry("look up").as_deref(), Some("tra cứu"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hostile_files_never_panic() {
        assert!(gunzip(&[0x1f, 0x8b, 8, 0xff]).is_err());
        assert!(gunzip(b"").is_err());
        let e = entries(b"word\0\xff\xff\xff\xff\xff\xff\xff\xffshort\0\x01", false);
        assert_eq!(e.len(), 1);
        let d = Dict { entries: e, data: b"abc".to_vec(), types: None };
        assert_eq!(d.lookup("word"), None);
        for chunk in [&b"m"[..], b"W\xff\xff\xff\xff", b"x<b", b"\0\0\0", b"mtext\0Wab"] {
            let _ = fields(chunk, None);
            let _ = fields(chunk, Some("mW"));
        }
        assert_eq!(plain("a <i>b</i> &amp; <c"), "a b & <c");
    }

    #[test]
    fn splits_the_selection_into_words() {
        assert_eq!(words("The cat's \"well-known\" toys, 2 of them.").collect::<Vec<_>>(), ["The", "cat's", "well-known", "toys", "2", "of", "them"]);
    }
}
