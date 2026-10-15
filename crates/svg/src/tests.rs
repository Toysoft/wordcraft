use super::*;

/// A 40 × 20 px SVG: red left half, blue right half.
fn halves() -> Vec<u8> {
    br##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20">
  <rect x="0" y="0" width="20" height="20" fill="#ff0000"/>
  <rect x="20" y="0" width="20" height="20" fill="#0000ff"/>
</svg>"##
        .to_vec()
}

fn px(r: &Raster, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * r.width + x) * 4) as usize;
    let p = r.rgba.get(i..i + 4).unwrap();
    [p[0], p[1], p[2], p[3]]
}

#[test]
fn recognises_svg_files() {
    assert!(is_svg(&halves()));
    assert!(is_svg(b"  <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>"));
    assert!(is_svg(b"\xEF\xBB\xBF<!-- c --><!DOCTYPE svg><svg width=\"1\" height=\"1\"/>"));
    assert!(!is_svg(b"\x89PNG\r\n\x1a\n"));
    assert!(!is_svg(b"<?xml version=\"1.0\"?><w:document/>"));
    assert!(!is_svg(b""));
    // Compressed SVG is refused (a gzip bomb could expand without bound).
    assert!(!is_svg(&[0x1f, 0x8b, 8, 0, 0, 0]));
}

#[test]
fn natural_size_is_css_pixels_at_96_ppi() {
    assert_eq!(natural_size(&halves()), Some((30.0, 15.0)));
    // A viewBox alone gives the size.
    let vb = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 48"/>"#;
    assert_eq!(natural_size(vb), Some((72.0, 36.0)));
    // Huge pictures are scaled down to 22 inches on the longer side.
    let big = br#"<svg xmlns="http://www.w3.org/2000/svg" width="96000" height="48000"/>"#;
    assert_eq!(natural_size(big), Some((1584.0, 792.0)));
    assert_eq!(natural_size(b"not svg"), None);
}

#[test]
fn renders_at_the_requested_resolution() {
    let svg = parse(&halves()).unwrap();
    assert_eq!(svg.size_px(), (40.0, 20.0));
    // Ten times the natural size: the edge between the halves is still one pixel sharp.
    let r = svg.render(400, 200, [10.0, 0.0, 0.0, 10.0, 0.0, 0.0]).unwrap();
    assert_eq!((r.width, r.height), (400, 200));
    assert_eq!(px(&r, 199, 100), [255, 0, 0, 255]);
    assert_eq!(px(&r, 200, 100), [0, 0, 255, 255]);
}

#[test]
fn fallback_png_is_twice_the_natural_size() {
    let png = fallback_png(&halves()).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
    // IHDR width and height.
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 80);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 40);
    let big = br#"<svg xmlns="http://www.w3.org/2000/svg" width="96000" height="48000"><rect width="10" height="10"/></svg>"#;
    let png = fallback_png(big).unwrap();
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), MAX_FALLBACK_SIDE);
}

#[test]
fn hostile_svgs_are_errors_not_panics() {
    for bad in [
        b"".as_slice(),
        b"<svg",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\"/>",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"-5\" height=\"NaN\"><rect/></svg>",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1e39\" height=\"1e39\"/>",
        b"\xff\xfe<\x00s\x00",
    ] {
        assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn deep_nesting_is_refused() {
    // Refused before usvg recurses into it.
    let deep = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}{}</svg>"#, "<g>".repeat(5000), "</g>".repeat(5000));
    assert_eq!(parse(deep.as_bytes()).err(), Some(Error::TooDeep));
}

#[test]
fn oversized_files_are_refused() {
    let mut huge = halves();
    huge.resize(MAX_SVG_BYTES + 1, b' ');
    assert_eq!(parse(&huge).err(), Some(Error::TooLarge));
}

#[test]
fn billion_laughs_is_bounded() {
    let lol = r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;"><!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;"><!ENTITY e "&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;"><!ENTITY f "&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;"><!ENTITY g "&f;&f;&f;&f;&f;&f;&f;&f;&f;&f;"><!ENTITY h "&g;&g;&g;&g;&g;&g;&g;&g;&g;&g;">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>&h;</text></svg>"#;
    assert!(parse(lol.as_bytes()).is_err(), "100 MB of expanded text is refused");
}

/// Review 1 #1: one wide entity referenced many times at depth 0 (which roxmltree allows without
/// limit) would expand to gigabytes. Scaled down: 64 KB × 2000 = 128 MB, refused before parsing.
#[test]
fn wide_entity_expansion_is_refused() {
    let doc = format!(
        r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY e "{}">]><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><text>{}</text></svg>"#,
        "a".repeat(16 << 10),
        "&e;".repeat(8000)
    );
    assert!(is_svg(doc.as_bytes()));
    let t = std::time::Instant::now();
    let r = parse(doc.as_bytes());
    assert!(matches!(&r, Err(Error::Parse(m)) if m.contains("entit")), "refused for its entities: {:?}", r.err());
    assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
}

/// Short entities, as Illustrator writes for its namespaces, still read.
#[test]
fn small_namespace_entities_are_accepted() {
    let doc = r##"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [
	<!ENTITY ns_svg "http://www.w3.org/2000/svg">
	<!ENTITY ns_xlink "http://www.w3.org/1999/xlink">
]>
<svg version="1.1" xmlns="&ns_svg;" xmlns:xlink="&ns_xlink;" width="20" height="10"><rect width="10" height="10" fill="#f00"/></svg>"##;
    let svg = parse(doc.as_bytes()).unwrap();
    assert_eq!(svg.size_px(), (20.0, 10.0));
}

/// Review 1 #2: the declaration scan looked for `[` to the end of the file at every `<!`, which
/// is quadratic: a few MB of `<!a>` never finished.
#[test]
fn declarations_scan_in_linear_time() {
    let doc = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}</svg>"#, "<!a>".repeat(1 << 16));
    let t = std::time::Instant::now();
    let _ = parse(doc.as_bytes());
    assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
    let prolog = format!("{}<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>", "<!DOCTYPE a>".repeat(5000));
    let t = std::time::Instant::now();
    for _ in 0..20 {
        let _ = is_svg(prolog.as_bytes());
    }
    assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
}

/// Review 1 #3: text in an SVG draws with the fonts documents use.
#[test]
fn text_is_drawn_with_document_fonts() {
    let doc = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><text x="5" y="50" font-family="Arial" font-size="50" fill="black">WW</text></svg>"#;
    let r = parse(doc).unwrap().render(100, 60, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap();
    let inked = r.rgba.chunks(4).filter(|p| p[3] > 128).count();
    assert!(inked > 200, "the text is drawn: {inked} inked pixels");
}

/// Review 1 #13: a prefixed root element is still an SVG.
#[test]
fn prefixed_svg_root_is_accepted() {
    let doc = br#"<svg:svg xmlns:svg="http://www.w3.org/2000/svg" width="10" height="10"><svg:rect width="10" height="10"/></svg:svg>"#;
    assert!(is_svg(doc));
    assert_eq!(natural_size(doc), Some((7.5, 7.5)));
    assert!(!is_svg(b"<w:svgish/>"));
}

#[test]
fn rasters_past_the_cap_are_refused() {
    let svg = parse(&halves()).unwrap();
    assert!(svg.render(100_000, 100_000, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
    assert!(svg.render(0, 10, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
    assert!(svg.render(10, 10, [f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0]).is_none());
}

#[test]
fn external_pictures_are_never_loaded() {
    // A real PNG on disk, linked by absolute path and by file: URL: neither is read.
    let dir = std::env::temp_dir().join(format!("wordcraft-svg-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png_path = dir.join("red.png");
    let png = fallback_png(&halves()).unwrap();
    std::fs::write(&png_path, &png).unwrap();
    let p = png_path.to_string_lossy();
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image href="{p}" width="10" height="20"/><image xlink:href="file://{p}" x="10" width="10" height="20"/></svg>"#
    );
    let svg = parse(doc.as_bytes()).unwrap();
    let r = svg.render(20, 20, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap();
    assert!(r.rgba.iter().all(|b| *b == 0), "nothing from the file is drawn");
    let _ = std::fs::remove_dir_all(&dir);
    // A data: picture is drawn.
    let b64 = base64_encode(&png);
    let inline = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image href="data:image/png;base64,{b64}" width="20" height="20"/></svg>"#
    );
    let r = parse(inline.as_bytes()).unwrap().render(20, 20, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]).unwrap();
    assert!(r.rgba.iter().any(|b| *b != 0), "the data: picture is drawn");
}

fn base64_encode(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for ch in b.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= ch.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[test]
fn chained_uses_and_nested_pictures_stay_within_the_stack() {
    // Each level wraps the previous one 200 deep through `use`: about 1000 levels once expanded.
    // Parsing and drawing must not overflow a test thread's 2 MB stack.
    let wrap = |inner: &str| format!("{}{inner}{}", "<g>".repeat(200), "</g>".repeat(200));
    let mut defs = format!(r#"<g id="l0">{}</g>"#, wrap(r##"<rect width="10" height="10" fill="#f00"/>"##));
    for i in 1..6 {
        defs.push_str(&format!(r##"<g id="l{i}">{}</g>"##, wrap(&format!(r##"<use href="#l{}"/>"##, i - 1))));
    }
    let doc = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs>{defs}</defs><use href="#l5"/></svg>"##);
    if let Ok(svg) = parse(doc.as_bytes()) {
        let _ = svg.render(10, 10, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }
    // SVG pictures inside SVG pictures (data: links), nested 12 deep.
    let mut svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#f00"/></svg>"##.to_string();
    for _ in 0..12 {
        let b64 = base64_encode(svg.as_bytes());
        svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}<image href="data:image/svg+xml;base64,{b64}" width="10" height="10"/>{}</svg>"#,
            "<g>".repeat(200),
            "</g>".repeat(200)
        );
    }
    if let Ok(s) = parse(svg.as_bytes()) {
        let _ = s.render(10, 10, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }
    // A gzip-compressed SVG inside a data: link is not expanded.
    let gz = base64_encode(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 1, 2, 3]);
    let doc = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><image href="data:image/svg+xml;base64,{gz}" width="10" height="10"/></svg>"#
    );
    assert!(parse(doc.as_bytes()).is_ok());
}
