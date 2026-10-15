//! WordCraft SVG pictures (#1910): reads an SVG file safely into a parsed tree (usvg), gives its
//! natural size, and rasterises it (resvg) at any resolution, so the canvas can draw it at device
//! resolution and the writers can store a PNG fallback where a format can't hold SVG.
//!
//! SVG is untrusted input. The reader never touches the file system or the network: `<image>`
//! links to files or URLs are ignored and only `data:` pictures are drawn; compressed SVG (SVGZ)
//! is refused, so a gzip bomb can't expand; the file size, element nesting and raster size are
//! capped, and entities that could expand past a bound are refused. Text in an SVG draws with the
//! fonts documents use (`wordcraft-fonts`, with Word's substitutions), loaded into usvg's otherwise
//! empty font database on demand; system fonts are never scanned for it.
//! Every failure is an [`Error`], never a panic.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod fonts;
#[cfg(test)]
mod tests;

pub use usvg;

/// Largest SVG file read (bytes).
pub const MAX_SVG_BYTES: usize = 32 << 20;
/// Deepest element nesting accepted (usvg allows 1024, which recursion on a small thread stack
/// can't always afford).
pub const MAX_DEPTH: usize = 256;
/// Most pixels one raster may have (8 megapixels: a 4K screen).
pub const MAX_RASTER_PIXELS: u64 = 8 * 1024 * 1024;
/// Longest side of a PNG fallback (pixels).
pub const MAX_FALLBACK_SIDE: u32 = 2048;

/// Why an SVG could not be read or drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not an SVG file (or a compressed one).
    NotSvg,
    /// Larger than [`MAX_SVG_BYTES`].
    TooLarge,
    /// Nested deeper than [`MAX_DEPTH`].
    TooDeep,
    /// The XML or SVG could not be parsed.
    Parse(String),
    /// A raster could not be made (empty or oversized).
    Raster,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotSvg => f.write_str("not an SVG picture"),
            Error::TooLarge => f.write_str("SVG picture is too large"),
            Error::TooDeep => f.write_str("SVG picture is nested too deeply"),
            Error::Parse(e) => write!(f, "can't read the SVG picture: {e}"),
            Error::Raster => f.write_str("can't draw the SVG picture"),
        }
    }
}

impl std::error::Error for Error {}

/// A parsed SVG picture.
pub struct Svg {
    tree: usvg::Tree,
}

/// A raster of an SVG: premultiplied RGBA8 rows, `width` × `height`.
#[derive(Debug, Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// How far into a file the `svg` root element is looked for (past the XML declaration, comments
/// and a document type declaration).
const SNIFF_BYTES: usize = 64 << 10;
/// Most nodes the XML parser builds (usvg's own limit is a million elements).
const MAX_XML_NODES: u32 = 4_000_000;
/// SVG pictures inside SVG pictures (`data:` links) are drawn this many levels deep.
const MAX_NESTED: u32 = 2;
/// Natural sizes are scaled down to this many points on the longer side (22 inches).
const MAX_NATURAL_PT: f32 = 1584.0;

/// Whether `bytes` look like an (uncompressed) SVG file: XML text whose first element is `svg`.
pub fn is_svg(bytes: &[u8]) -> bool {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    // Cheap first: a bitmap (PNG, JPEG…) doesn't start with `<`.
    if b.iter().find(|c| !c.is_ascii_whitespace()) != Some(&b'<') {
        return false;
    }
    let head = String::from_utf8_lossy(b.get(..b.len().min(SNIFF_BYTES)).unwrap_or(b));
    let mut t = head.trim_start();
    // Skip the prolog: the XML declaration, processing instructions, comments and a DOCTYPE.
    loop {
        let skip = if t.starts_with("<?") {
            t.find("?>").map(|i| i + 2)
        } else if t.starts_with("<!--") {
            t.find("-->").map(|i| i + 3)
        } else if t.get(..9).is_some_and(|d| d.eq_ignore_ascii_case("<!DOCTYPE")) {
            // An internal subset (a `[` before the first `>`) ends at `]>`. Searches stay within
            // the declaration, so many declarations scan in linear time.
            t.find('>').and_then(|close| match t.get(..close).and_then(|d| d.find('[')) {
                Some(open) => t.get(open..).and_then(|r| r.find("]>")).map(|i| open + i + 2),
                None => Some(close + 1),
            })
        } else {
            break;
        };
        match skip.and_then(|i| t.get(i..)) {
            Some(rest) => t = rest.trim_start(),
            None => return false,
        }
    }
    // The root element's name: `svg`, or `prefix:svg`.
    let name: &str = t.strip_prefix('<').map_or("", |r| r.split(|c: char| c.is_whitespace() || c == '>' || c == '/').next().unwrap_or(""));
    let local = name.rsplit_once(':').map_or(name, |(_, l)| l);
    local == "svg"
}

/// Parse an SVG file safely (see the crate docs).
pub fn parse(bytes: &[u8]) -> Result<Svg, Error> {
    parse_nested(bytes, 0)
}

/// [`parse`] for a picture `level` SVG pictures deep.
fn parse_nested(bytes: &[u8], level: u32) -> Result<Svg, Error> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(Error::TooLarge);
    }
    if !is_svg(bytes) {
        return Err(Error::NotSvg);
    }
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    let text = std::str::from_utf8(b).map_err(|e| Error::Parse(e.to_string()))?;
    check_nesting(text)?;
    let xml_opts = usvg::roxmltree::ParsingOptions { allow_dtd: true, nodes_limit: MAX_XML_NODES, ..Default::default() };
    let xml = usvg::roxmltree::Document::parse_with_options(text, xml_opts).map_err(|e| Error::Parse(e.to_string()))?;
    // Element nesting, measured without recursion (parents come before their children).
    let mut depth: Vec<u16> = Vec::new();
    for n in xml.descendants() {
        let d = n.parent().and_then(|p| depth.get(p.id().get_usize()).copied()).map_or(0, |d| d.saturating_add(1));
        if d as usize > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        let i = n.id().get_usize();
        if depth.len() <= i {
            depth.resize(i + 1, 0);
        }
        if let Some(slot) = depth.get_mut(i) {
            *slot = d;
        }
    }
    let opts = usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(move |mime: &str, data: std::sync::Arc<Vec<u8>>, _: &usvg::Options| match mime {
                "image/jpg" | "image/jpeg" => Some(usvg::ImageKind::JPEG(data)),
                "image/png" => Some(usvg::ImageKind::PNG(data)),
                "image/gif" => Some(usvg::ImageKind::GIF(data)),
                "image/webp" => Some(usvg::ImageKind::WEBP(data)),
                // Our own reader, so a nested picture gets the same checks, and nesting is bounded.
                "image/svg+xml" if level < MAX_NESTED => parse_nested(&data, level + 1).ok().map(|s| usvg::ImageKind::SVG(s.tree)),
                _ => None,
            }),
            // Files and URLs are never read.
            resolve_string: Box::new(|_: &str, _: &usvg::Options| None),
        },
        font_resolver: fonts::resolver(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_xmltree(&xml, &opts).map_err(|e| Error::Parse(e.to_string()))?;
    Ok(Svg { tree })
}

/// Most entity declarations a file may have.
const MAX_ENTITIES: usize = 4096;
/// Most character data (bytes, entities expanded) inside text elements: past it usvg would shape
/// and lay out megabytes of text on the thread that opens the picture.
const MAX_TEXT_BYTES: u64 = 1 << 20;
/// Most text a picture may draw once `<use>` copies are counted (text bytes × (1 + uses)).
const MAX_TEXT_DRAWN: u64 = 16 << 20;
/// Most text entity references may expand to (bytes): the longest entity value (with the
/// entities it references expanded) times the number of `&` in the file.
const MAX_ENTITY_EXPANSION: u64 = 16 << 20;

/// What the pre-parse scan has seen of the file's entities.
struct Entities<'a> {
    /// Expanded length of each declared entity, by name.
    sizes: std::collections::HashMap<&'a [u8], u64>,
    /// The longest expanded value.
    longest: u64,
    /// Declarations seen.
    count: usize,
    /// Number of `&` in the file, counted the first time a declaration needs it.
    refs: Option<u64>,
}

fn refused<T>(why: &str) -> Result<T, Error> {
    Err(Error::Parse(why.to_string()))
}

/// Refuses, before the XML parser (which recurses on nesting) or usvg see it: markup nested
/// deeper than [`MAX_DEPTH`]; entities that hold markup (which could nest elements past the scan),
/// that could expand past [`MAX_ENTITY_EXPANSION`], or more than [`MAX_ENTITIES`] of them; and more
/// than [`MAX_TEXT_BYTES`] of text in text elements. One pass over the tags, linear in the file.
fn check_nesting(text: &str) -> Result<(), Error> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    let mut ents = Entities { sizes: std::collections::HashMap::new(), longest: 0, count: 0, refs: None };
    // Depth of the open text element (its content is shaped), text bytes so far, `<use>` count.
    let mut text_at: Option<usize> = None;
    let mut text_bytes = 0u64;
    let mut uses = 0u64;
    let find = |from: usize, pat: &[u8]| b.get(from..).and_then(|rest| rest.windows(pat.len()).position(|w| w == pat)).map(|p| from + p);
    while let Some(lt) = b.get(i..).and_then(|rest| rest.iter().position(|c| *c == b'<')).map(|p| i + p) {
        if text_at.is_some() {
            text_bytes = text_bytes.saturating_add(char_data_size(b.get(i..lt).unwrap_or_default(), &ents));
            if text_bytes > MAX_TEXT_BYTES {
                return refused("SVG text this long is not supported");
            }
        }
        let rest = b.get(lt..).unwrap_or_default();
        i = if rest.starts_with(b"<!--") {
            find(lt + 4, b"-->").map_or(b.len(), |e| e + 3)
        } else if rest.starts_with(b"<![CDATA[") {
            let end = find(lt + 9, b"]]>").unwrap_or(b.len());
            if text_at.is_some() {
                text_bytes = text_bytes.saturating_add(end.saturating_sub(lt + 9) as u64);
                if text_bytes > MAX_TEXT_BYTES {
                    return refused("SVG text this long is not supported");
                }
            }
            end.saturating_add(3)
        } else if rest.starts_with(b"<?") {
            find(lt + 2, b"?>").map_or(b.len(), |e| e + 2)
        } else if rest.starts_with(b"<!") {
            // A declaration; a DOCTYPE's internal subset (a `[` before its first `>`) ends at
            // `]>`. The `[` is looked for within the declaration only, so a run of declarations
            // scans in linear time.
            let close = find(lt, b">").unwrap_or(b.len());
            let open = b.get(lt..close).and_then(|d| d.iter().position(|c| *c == b'[')).map(|p| lt + p);
            let end = match open {
                Some(open) => find(open, b"]>").map_or(b.len(), |e| e + 2),
                None => close.saturating_add(1),
            };
            let decl = b.get(lt..end.min(b.len())).unwrap_or_default();
            if decl.windows(8).any(|w| w == b"<!ENTITY") {
                check_entities(decl, b, &mut ents)?;
            }
            end
        } else if rest.starts_with(b"</") {
            if text_at == Some(depth) {
                text_at = None;
            }
            depth = depth.saturating_sub(1);
            find(lt, b">").map_or(b.len(), |e| e + 1)
        } else {
            // A start tag: its end is the first `>` outside a quoted attribute value.
            let mut j = lt + 1;
            let mut quote = None;
            while let Some(&c) = b.get(j) {
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == b'"' || c == b'\'' => quote = Some(c),
                    None if c == b'>' => break,
                    None => {}
                }
                j += 1;
            }
            let name = rest.get(1..).unwrap_or_default();
            let name =
                name.get(..name.iter().position(|c| c.is_ascii_whitespace() || *c == b'>' || *c == b'/').unwrap_or(name.len())).unwrap_or_default();
            let local = name.rsplit(|c| *c == b':').next().unwrap_or_default();
            if local == b"use" {
                uses = uses.saturating_add(1);
            }
            if b.get(j.saturating_sub(1)) != Some(&b'/') {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Error::TooDeep);
                }
                if local == b"text" && text_at.is_none() {
                    text_at = Some(depth);
                }
            }
            j.saturating_add(1)
        };
    }
    if text_bytes.saturating_mul(uses.saturating_add(1)) > MAX_TEXT_DRAWN {
        return refused("SVG text this long is not supported");
    }
    Ok(())
}

/// The length of character data once its entity references are expanded.
fn char_data_size(data: &[u8], ents: &Entities) -> u64 {
    let mut size = data.len() as u64;
    if ents.sizes.is_empty() {
        return size;
    }
    let mut k = 0usize;
    while let Some(amp) = data.get(k..).and_then(|r| r.iter().position(|c| *c == b'&')).map(|p| k + p) {
        // The name runs to the `;` (or the next `&` in a malformed reference).
        let semi = data.get(amp + 1..).and_then(|r| r.iter().position(|c| *c == b';' || *c == b'&')).map_or(data.len(), |p| amp + 1 + p);
        if let Some(n) = data.get(amp + 1..semi).and_then(|name| ents.sizes.get(name)) {
            size = size.saturating_add(*n);
        }
        k = if data.get(semi) == Some(&b';') { semi + 1 } else { semi };
    }
    size
}

/// Records the entity declarations in `decl` (a part of `file`). Refuses a value holding markup,
/// more than [`MAX_ENTITIES`] declarations, and values that could expand past
/// [`MAX_ENTITY_EXPANSION`] given the references in the file. The XML parser only limits
/// references nested in entities; any number at the top level each expand to a full copy.
fn check_entities<'a>(decl: &'a [u8], file: &[u8], ents: &mut Entities<'a>) -> Result<(), Error> {
    let mut i = 0usize;
    while let Some(at) = decl.get(i..).and_then(|r| r.windows(8).position(|w| w == b"<!ENTITY")).map(|p| i + p + 8) {
        ents.count += 1;
        if ents.count > MAX_ENTITIES {
            return refused("this many entities are not supported");
        }
        let rest = decl.get(at..).unwrap_or_default();
        // `<!ENTITY [%] name "value">` (an external entity has no quoted value of its own here).
        let name_start = rest.iter().position(|c| !c.is_ascii_whitespace() && *c != b'%').unwrap_or(rest.len());
        let name_len = rest.get(name_start..).unwrap_or_default().iter().position(|c| c.is_ascii_whitespace()).unwrap_or(0);
        let name = rest.get(name_start..name_start + name_len).unwrap_or_default();
        let after = name_start + name_len;
        let Some(q) = rest.get(after..).and_then(|r| r.iter().position(|c| *c == b'"' || *c == b'\'' || *c == b'>')).map(|p| after + p) else {
            break;
        };
        let quote = rest.get(q).copied().unwrap_or(b'>');
        if quote == b'>' {
            i = at + q + 1;
            continue;
        }
        let vlen = rest.get(q + 1..).and_then(|r| r.iter().position(|c| *c == quote)).unwrap_or(0);
        let value = rest.get(q + 1..q + 1 + vlen).unwrap_or_default();
        if value.contains(&b'<') {
            return refused("entities holding markup are not supported");
        }
        // The value's expanded length: its text plus the expansions of the entities it names.
        let size = char_data_size(value, ents);
        ents.sizes.entry(name).or_insert(size);
        ents.longest = ents.longest.max(size);
        let refs = *ents.refs.get_or_insert_with(|| file.iter().filter(|c| **c == b'&').count() as u64);
        if ents.longest.saturating_mul(refs) > MAX_ENTITY_EXPANSION {
            return refused("entities expanding this far are not supported");
        }
        i = at + q + 1 + vlen;
    }
    Ok(())
}

/// The natural size of an SVG picture in points (its width and height, or its `viewBox`, as CSS
/// pixels at 96 per inch), or None when it isn't a readable SVG. Very large pictures are scaled
/// down to 22 inches on the longer side, keeping the shape.
pub fn natural_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let (w, h) = parse(bytes).ok()?.size_px();
    let (w, h) = (w * 0.75, h * 0.75);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let k = (MAX_NATURAL_PT / w.max(h)).min(1.0);
    Some((w * k, h * k))
}

/// A PNG of the SVG at twice its natural size (capped at [`MAX_FALLBACK_SIDE`] on the longer
/// side), for formats that can't hold SVG.
pub fn fallback_png(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let svg = parse(bytes)?;
    let (w, h) = svg.size_px();
    let k = (2.0f32).min(MAX_FALLBACK_SIDE as f32 / w.max(h));
    let (pw, ph) = (((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1));
    let pm = svg.pixmap(pw, ph, [pw as f32 / w, 0.0, 0.0, ph as f32 / h, 0.0, 0.0]).ok_or(Error::Raster)?;
    pm.encode_png().map_err(|_| Error::Raster)
}

impl Svg {
    /// The parsed tree (for drawing it as vectors, e.g. into a PDF).
    pub fn tree(&self) -> &usvg::Tree {
        &self.tree
    }

    /// The picture's size in CSS pixels (user units).
    pub fn size_px(&self) -> (f32, f32) {
        let s = self.tree.size();
        (s.width(), s.height())
    }

    /// A `width` × `height` raster of the picture drawn with `transform` (`[sx, ky, kx, sy, tx, ty]`
    /// from SVG user units to raster pixels). None when the raster would be empty or larger than
    /// [`MAX_RASTER_PIXELS`].
    pub fn render(&self, width: u32, height: u32, transform: [f32; 6]) -> Option<Raster> {
        let pm = self.pixmap(width, height, transform)?;
        let (width, height) = (pm.width(), pm.height());
        Some(Raster { width, height, rgba: pm.take() })
    }

    fn pixmap(&self, width: u32, height: u32, transform: [f32; 6]) -> Option<resvg::tiny_skia::Pixmap> {
        if width == 0 || height == 0 || (width as u64) * (height as u64) > MAX_RASTER_PIXELS || !transform.iter().all(|v| v.is_finite()) {
            return None;
        }
        let mut pm = resvg::tiny_skia::Pixmap::new(width, height)?;
        let [sx, ky, kx, sy, tx, ty] = transform;
        resvg::render(&self.tree, resvg::tiny_skia::Transform::from_row(sx, ky, kx, sy, tx, ty), &mut pm.as_mut());
        Some(pm)
    }
}
