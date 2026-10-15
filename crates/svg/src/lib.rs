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

use std::sync::atomic::{AtomicU64, Ordering};

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
    let budget = Budget::default();
    parse_nested(bytes, 0, &budget)
}

/// What a picture and the SVG pictures nested in it may hold between them (#1910 review 3).
#[derive(Default)]
struct Budget {
    /// Text bytes in text elements so far.
    text: AtomicU64,
    /// Text bytes drawn so far, `<use>` copies counted.
    drawn: AtomicU64,
    /// Bytes of nested pictures read so far (usvg reads a nested picture once per copy of it).
    nested: AtomicU64,
    /// Why a nested picture went past the caps: the whole picture is refused.
    over: std::sync::OnceLock<&'static str>,
}

/// [`parse`] for a picture `level` SVG pictures deep, sharing `budget` with the whole picture.
fn parse_nested(bytes: &[u8], level: u32, budget: &Budget) -> Result<Svg, Error> {
    if bytes.len() > MAX_SVG_BYTES {
        return Err(Error::TooLarge);
    }
    if !is_svg(bytes) {
        return Err(Error::NotSvg);
    }
    if level > 0 {
        let len = bytes.len() as u64;
        if budget.nested.fetch_add(len, Ordering::Relaxed).saturating_add(len) > MAX_SVG_BYTES as u64 {
            return budget.refuse(NESTED_TOO_LARGE);
        }
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
                "image/svg+xml" if level < MAX_NESTED => parse_nested(&data, level + 1, budget).ok().map(|s| usvg::ImageKind::SVG(s.tree)),
                _ => None,
            }),
            // Files and URLs are never read.
            resolve_string: Box::new(|_: &str, _: &usvg::Options| None),
        },
        font_resolver: fonts::resolver(),
        ..usvg::Options::default()
    };
    check_copies(&xml, budget)?;
    let tree = usvg::Tree::from_xmltree(&xml, &opts).map_err(|e| Error::Parse(e.to_string()))?;
    if let Some(why) = budget.over.get() {
        return refused(why);
    }
    Ok(Svg { tree })
}

/// Most entity declarations a file may have.
const MAX_ENTITIES: usize = 4096;
/// Most character data (bytes, entities expanded) inside text elements: past it usvg would shape
/// and lay out megabytes of text on the thread that opens the picture.
const MAX_TEXT_BYTES: u64 = 1 << 20;
/// Most text a picture may draw once `<use>` copies (and copies of copies) are counted.
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

const TEXT_TOO_LONG: &str = "SVG text this long is not supported";
const NESTED_TOO_LARGE: &str = "SVG with this many nested pictures is not supported";

impl Budget {
    /// Refuses this picture, and with it the pictures it is nested in.
    fn refuse<T>(&self, why: &'static str) -> Result<T, Error> {
        let _ = self.over.set(why);
        refused(why)
    }
}

fn refused<T>(why: &str) -> Result<T, Error> {
    Err(Error::Parse(why.to_string()))
}

/// Refuses, before the XML parser (which recurses on nesting) sees it: markup nested deeper than
/// [`MAX_DEPTH`]; entities that hold markup (which could nest elements past the scan), that could
/// expand past [`MAX_ENTITY_EXPANSION`], or more than [`MAX_ENTITIES`] of them. One pass over the
/// tags, linear in the file.
fn check_nesting(text: &str) -> Result<(), Error> {
    let b = text.as_bytes();
    let mut i = 0usize;
    let mut ents = Entities { sizes: std::collections::HashMap::new(), longest: 0, count: 0, refs: None };
    let mut depth = 0usize;
    let find = |from: usize, pat: &[u8]| b.get(from..).and_then(|rest| rest.windows(pat.len()).position(|w| w == pat)).map(|p| from + p);
    while let Some(lt) = b.get(i..).and_then(|rest| rest.iter().position(|c| *c == b'<')).map(|p| i + p) {
        let rest = b.get(lt..).unwrap_or_default();
        i = if rest.starts_with(b"<!--") {
            find(lt + 4, b"-->").map_or(b.len(), |e| e + 3)
        } else if rest.starts_with(b"<![CDATA[") {
            find(lt + 9, b"]]>").map_or(b.len(), |e| e + 3)
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
            if b.get(j.saturating_sub(1)) != Some(&b'/') {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(Error::TooDeep);
                }
            }
            j.saturating_add(1)
        };
    }
    Ok(())
}

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
/// Most work `<tref>` links may cost: usvg finds each one's target by walking the file's nodes, so
/// the links resolved (copies counted) times the nodes in the file.
const MAX_TREF_WORK: u64 = 1 << 26;
const ENDLESS_COPIES: &str = "SVG whose copies of itself never end is not supported";
const TREFS_TOO_MANY: &str = "SVG with this many text references is not supported";

/// Refuses, before usvg builds the picture from the parsed XML: more than [`MAX_TEXT_BYTES`] of
/// text in text elements, and more than [`MAX_TEXT_DRAWN`] once usvg's copies are counted
/// ([`copies`]), counting what `budget` holds already (the pictures this one is nested in, and
/// those nested before it); `<use>` links that copy without end; and `<tref>` links that would
/// cost more than [`MAX_TREF_WORK`] to find.
fn check_copies(xml: &usvg::roxmltree::Document, budget: &Budget) -> Result<(), Error> {
    // The text as written: each outermost text element's text, once.
    let written = xml
        .descendants()
        .filter(|n| kind(*n) == Kind::Text && !n.ancestors().skip(1).any(|a| kind(a) == Kind::Text))
        .flat_map(|n| n.descendants())
        .filter_map(|n| n.text().filter(|_| n.is_text()))
        .fold(0u64, |a, t| a.saturating_add(t.len() as u64));
    if budget.text.fetch_add(written, Ordering::Relaxed).saturating_add(written) > MAX_TEXT_BYTES {
        return budget.refuse(TEXT_TOO_LONG);
    }
    let c = copies(xml);
    if c.endless {
        return budget.refuse(ENDLESS_COPIES);
    }
    if budget.drawn.fetch_add(c.bytes, Ordering::Relaxed).saturating_add(c.bytes) > MAX_TEXT_DRAWN {
        return budget.refuse(TEXT_TOO_LONG);
    }
    let nodes = xml.descendants().count() as u64;
    if c.trefs.saturating_mul(nodes) > MAX_TREF_WORK {
        return budget.refuse(TREFS_TOO_MANY);
    }
    Ok(())
}

/// What usvg builds of a picture's text, `<use>` copies (and copies of copies) included.
#[derive(Clone, Copy, Default, Debug)]
struct Copies {
    /// Text elements built.
    texts: u64,
    /// Bytes of text in them, `<tref>` text included.
    bytes: u64,
    /// `<tref>` links resolved.
    trefs: u64,
    /// The copies never end (usvg would give up deep down, after building a great deal).
    endless: bool,
}

impl Copies {
    const ENDLESS: Copies = Copies { texts: u64::MAX, bytes: u64::MAX, trefs: u64::MAX, endless: true };

    fn add(self, o: Copies) -> Copies {
        Copies {
            texts: self.texts.saturating_add(o.texts),
            bytes: self.bytes.saturating_add(o.bytes),
            trefs: self.trefs.saturating_add(o.trefs),
            endless: self.endless || o.endless,
        }
    }
}

/// How usvg treats an XML node when it builds its tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Not built, nor anything inside it: text and comments, elements outside the SVG namespace,
    /// `<style>`.
    Skip,
    /// A text element: its text (and `<tref>` text) is built, its children are not walked again.
    Text,
    /// A `<use>`: builds a copy of the element it links to.
    Use,
    /// Any other SVG element, built with its children. (usvg also skips SVG elements it doesn't
    /// know; they are counted here, which can only overcount.)
    Group,
}

fn kind(n: usvg::roxmltree::Node) -> Kind {
    if !n.is_element() || !matches!(n.tag_name().namespace(), None | Some(SVG_NS)) {
        return Kind::Skip;
    }
    match n.tag_name().name() {
        "style" => Kind::Skip,
        "text" => Kind::Text,
        "use" => Kind::Use,
        _ => Kind::Group,
    }
}

/// The element a `<use>` or `<tref>` links to, read as usvg reads it (with the same calls):
/// `xlink:href`, else the first attribute whose local name is `href` (roxmltree looks a name
/// without a namespace up in any namespace), holding `#id` with spaces around it allowed
/// (svgtypes' IRI), naming the first element with that `id`.
fn link<'a, 'i>(
    n: usvg::roxmltree::Node<'a, 'i>,
    ids: &std::collections::HashMap<&str, usvg::roxmltree::Node<'a, 'i>>,
) -> Option<usvg::roxmltree::Node<'a, 'i>> {
    let value = n.attribute((XLINK_NS, "href")).or_else(|| n.attribute("href"))?;
    let id = svgtypes::IRI::from_str(value).ok()?.0;
    ids.get(id).copied()
}

/// What usvg builds of the text in `xml`, following its rules for `<use>` (usvg 0.47,
/// `svgtree/parse.rs`): a `<use>` builds a copy of the element it links to unless it links to
/// itself, to a `<use>` that links back to it, to an element outside the SVG namespace, or to an
/// element holding a `<use>` that links to that element or to this `<use>`. Copies hold copies,
/// so the text adds up along every path; work shared by many copies is done once (each node is
/// worked out once, with an explicit stack: linear, no recursion), and copies that lead back to
/// themselves are endless. Where usvg's rules are not mirrored (a `<use>` without the SVG
/// namespace that links back to its copy), this counts more, never less.
fn copies<'a, 'i>(xml: &'a usvg::roxmltree::Document<'i>) -> Copies {
    type Node<'a, 'i> = usvg::roxmltree::Node<'a, 'i>;
    let all: Vec<Node> = xml.descendants().collect();
    let len = all.iter().map(|n| n.id().get_usize() + 1).max().unwrap_or(0);
    let at = |n: Node| n.id().get_usize();
    let mut ids = std::collections::HashMap::new();
    for n in &all {
        if let Some(id) = n.attribute("id") {
            ids.entry(id).or_insert(*n);
        }
    }
    // Each node's last descendant (for "is inside"), and the text inside it.
    let mut last = vec![0usize; len];
    let mut text = vec![0u64; len];
    for n in all.iter().rev() {
        let i = at(*n);
        let l = n.last_child().and_then(|c| last.get(at(c)).copied()).unwrap_or(i);
        let own = if n.is_text() { n.text().map_or(0, |t| t.len() as u64) } else { 0 };
        let t = n.children().fold(own, |a, c| a.saturating_add(text.get(at(c)).copied().unwrap_or(0)));
        if let (Some(ls), Some(ts)) = (last.get_mut(i), text.get_mut(i)) {
            *ls = l;
            *ts = t;
        }
    }
    let inside = |a: Node, d: Node| at(a) < at(d) && last.get(at(a)).is_some_and(|l| at(d) <= *l);
    let svg_element = |n: Node| n.is_element() && matches!(n.tag_name().namespace(), None | Some(SVG_NS));
    // usvg skips a `<use>` whose target holds a `<use>` (in the SVG namespace) linking to that
    // target (LOOPS on the target) or to the `<use>` itself (LOOPS_BACK on the `<use>`).
    const LOOPS: u8 = 1;
    const LOOPS_BACK: u8 = 2;
    let mut flags = vec![0u8; len];
    for d in &all {
        if !(d.is_element() && d.tag_name().namespace() == Some(SVG_NS) && d.tag_name().name() == "use") {
            continue;
        }
        let Some(x) = link(*d, &ids) else { continue };
        if inside(x, *d)
            && let Some(f) = flags.get_mut(at(x))
        {
            *f |= LOOPS;
        }
        if kind(x) == Kind::Use
            && link(x, &ids).is_some_and(|y| inside(y, *d))
            && let Some(f) = flags.get_mut(at(x))
        {
            *f |= LOOPS_BACK;
        }
    }
    let flag = |n: Node, f: u8| flags.get(at(n)).is_some_and(|v| v & f != 0);
    // What a `<use>` copies.
    let copied = |u: Node<'a, 'i>| -> Option<Node<'a, 'i>> {
        let t = link(u, &ids)?;
        let skipped = t == u
            || !svg_element(t)
            || flag(u, LOOPS_BACK)
            || flag(t, LOOPS)
            // A `<use>` of a `<use>` of this one: the inner one links to its own copy's origin.
            || (kind(t) == Kind::Use && link(t, &ids) == Some(u));
        (!skipped).then_some(t)
    };
    // `<tref>` text and links inside each text element.
    let mut trefs: std::collections::HashMap<usize, Copies> = std::collections::HashMap::new();
    for r in &all {
        if !(svg_element(*r) && r.tag_name().name() == "tref" && (r.has_attribute((XLINK_NS, "href")) || r.has_attribute("href"))) {
            continue;
        }
        let bytes = link(*r, &ids).filter(|t| svg_element(*t)).and_then(|t| text.get(at(t)).copied()).unwrap_or(0);
        for a in r.ancestors().skip(1).filter(|a| kind(*a) == Kind::Text) {
            let e = trefs.entry(at(a)).or_default();
            *e = e.add(Copies { texts: 0, bytes, trefs: 1, endless: false });
        }
    }
    // What each node builds where it stands, worked out after what it builds from.
    let own = |n: Node| match kind(n) {
        Kind::Text => {
            let t = Copies { texts: 1, bytes: text.get(at(n)).copied().unwrap_or(0), trefs: 0, endless: false };
            t.add(trefs.get(&at(n)).copied().unwrap_or_default())
        }
        _ => Copies::default(),
    };
    let from = |n: Node<'a, 'i>| -> Vec<Node<'a, 'i>> {
        match kind(n) {
            _ if n == xml.root() => n.children().filter(|c| kind(*c) != Kind::Skip).collect(),
            Kind::Group => n.children().filter(|c| kind(*c) != Kind::Skip).collect(),
            Kind::Use => copied(n).into_iter().collect(),
            Kind::Text | Kind::Skip => Vec::new(),
        }
    };
    // 0: not reached, 1: being worked out (on the walk's path), 2: worked out.
    let mut state = vec![0u8; len];
    let mut built = vec![Copies::default(); len];
    let mut stack = vec![xml.root()];
    while let Some(&n) = stack.last() {
        let i = at(n);
        match state.get(i).copied() {
            Some(0) => {
                if let Some(s) = state.get_mut(i) {
                    *s = 1;
                }
                stack.extend(from(n).into_iter().filter(|d| state.get(at(*d)) == Some(&0)));
            }
            Some(1) => {
                // What it builds from is worked out now, or still on the path: a copy of itself.
                let c = from(n).into_iter().fold(own(n), |a, d| {
                    a.add(if state.get(at(d)) == Some(&2) { built.get(at(d)).copied().unwrap_or_default() } else { Copies::ENDLESS })
                });
                if let (Some(b), Some(s)) = (built.get_mut(i), state.get_mut(i)) {
                    *b = c;
                    *s = 2;
                }
                stack.pop();
            }
            _ => {
                stack.pop();
            }
        }
    }
    built.get(at(xml.root())).copied().unwrap_or_default()
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
