//! A field nested in another field's code (`{ IF { REF amount } = 0 … }`) survives a round trip
//! with its own begin, code, result and end, and the outer field keeps its cached result (#1671).
//! All markup here is synthetic, written from ECMA-376 Part 1 §17.16 (complex fields nest via
//! `w:fldChar` begin / separate / end).

use std::io::Write;

use wordcraft_doc::StoryRef;
use wordcraft_docx::{read, write};

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
const ROOT_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

fn package(body: &str) -> Vec<u8> {
    let doc = format!(r#"<w:document {NS}><w:body>{body}</w:body></w:document>"#);
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in [("_rels/.rels", ROOT_RELS.as_bytes()), ("word/document.xml", doc.as_bytes())] {
        zw.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn document_xml(docx: &[u8]) -> String {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut s).unwrap();
    s
}

fn resaved(body: &str) -> String {
    document_xml(&write(&read(&package(body)).unwrap()).unwrap())
}

fn t(s: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{s}</w:t></w:r>"#)
}
fn fld(kind: &str) -> String {
    format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#)
}
fn instr(s: &str) -> String {
    format!(r#"<w:r><w:instrText xml:space="preserve">{s}</w:instrText></w:r>"#)
}

/// `{ REF amount }` with cached result `0`.
fn ref_amount() -> String {
    format!("{}{}{}{}{}", fld("begin"), instr(" REF amount "), fld("separate"), t("0"), fld("end"))
}

/// The issue's fixture: bookmark `amount` = 0; `{ IF { REF amount } = 0 "…" "…" }`;
/// `{ = { REF amount } + 5 }`.
fn fixture() -> String {
    format!(
        concat!(
            r#"<w:p><w:bookmarkStart w:id="0" w:name="amount"/>{}<w:bookmarkEnd w:id="0"/></w:p>"#,
            "<w:p>{}{}{}{}{}{}{}</w:p>",
            "<w:p>{}{}{}{}{}{}{}</w:p>",
        ),
        t("0"),
        fld("begin"),
        instr(" IF "),
        ref_amount(),
        instr(r#" = 0 "amount is zero" "amount is not zero" "#),
        fld("separate"),
        t("amount is zero"),
        fld("end"),
        fld("begin"),
        instr(" = "),
        ref_amount(),
        instr(" + 5 "),
        fld("separate"),
        t("5"),
        fld("end"),
    )
}

/// The structure of every field in `xml`, in document order: `[` for begin, `|` for separate,
/// `]` for end, and each `w:instrText`'s text in between.
fn field_marks(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    loop {
        let next = ["<w:fldChar ", "<w:instrText"].iter().filter_map(|m| rest.find(m).map(|i| (i, *m))).min();
        let Some((at, m)) = next else { break };
        rest = &rest[at..];
        if m == "<w:fldChar " {
            let end = rest.find('>').unwrap();
            let tag = &rest[..end];
            out.push(if tag.contains("\"begin\"") {
                '['
            } else if tag.contains("\"separate\"") {
                '|'
            } else {
                ']'
            });
            rest = &rest[end..];
        } else {
            let open = rest.find('>').unwrap() + 1;
            let close = rest.find("</w:instrText>").unwrap();
            out.push_str(&rest[open..close].trim().replace("&quot;", "\""));
            rest = &rest[close..];
        }
    }
    out
}

#[test]
fn nested_field_codes_survive_a_round_trip() {
    let xml = resaved(&fixture());
    assert_eq!(xml.matches(r#"w:fldCharType="begin""#).count(), 4, "{xml}");
    assert_eq!(field_marks(&xml), concat!(r#"[IF[REF amount|]= 0 "amount is zero" "amount is not zero"|]"#, "[=[REF amount|]+ 5|]"), "{xml}");
    // The nested REF fields keep their cached results.
    assert_eq!(xml.matches(r#"<w:t xml:space="preserve">0</w:t>"#).count(), 3, "{xml}");
}

#[test]
fn nested_field_outer_keeps_its_cached_result_as_displayed_text() {
    let doc = read(&package(&fixture())).unwrap();
    assert_eq!(doc.plain_text(StoryRef::Body), "0\namount is zero\n5");
    // And the result is still there after saving and reading back.
    let again = read(&write(&doc).unwrap()).unwrap();
    assert_eq!(again.plain_text(StoryRef::Body), "0\namount is zero\n5");
    // A second round trip keeps the nesting too.
    let xml = document_xml(&write(&again).unwrap());
    assert_eq!(xml.matches(r#"w:fldCharType="begin""#).count(), 4, "{xml}");
}

#[test]
fn nested_fields_three_deep_survive() {
    let inner = format!("{}{}{}{}{}", fld("begin"), instr(" PAGE "), fld("separate"), t("1"), fld("end"));
    let mid = format!("{}{}{}{}{}{}", fld("begin"), instr(" = "), inner, instr(" + 1 "), fld("separate"), t("2"));
    let body = format!(
        "<w:p>{}{}{}{}{}{}{}{}</w:p>",
        fld("begin"),
        instr(" IF "),
        mid,
        fld("end"),
        instr(r#" = 2 "two" "other" "#),
        fld("separate"),
        t("two"),
        fld("end")
    );
    let xml = resaved(&body);
    assert_eq!(field_marks(&xml), r#"[IF[=[PAGE|]+ 1|]= 2 "two" "other"|]"#, "{xml}");
}

#[test]
fn a_field_without_nesting_is_written_as_before() {
    let body = format!("<w:p>{}{}{}{}{}</w:p>", fld("begin"), instr(" REF amount "), fld("separate"), t("0"), fld("end"));
    let xml = resaved(&body);
    assert_eq!(field_marks(&xml), "[REF amount|]", "{xml}");
    // The writer's own form for a field: a space-padded code in one instrText.
    assert!(xml.contains(r#"<w:instrText xml:space="preserve"> REF amount </w:instrText>"#), "{xml}");
}

#[test]
fn hostile_nested_fields_never_panic() {
    // Deeply nested begins in a code, never ended.
    let deep: String = (0..200).map(|_| format!("{}{}", fld("begin"), instr(" IF "))).collect();
    let _ = read(&package(&format!("<w:p>{deep}</w:p>"))).map(|d| write(&d));
    // Deeply nested and properly ended.
    let mut nested = format!("{}{}{}", fld("begin"), instr(" PAGE "), fld("end"));
    for _ in 0..100 {
        nested = format!("{}{}{}{}{}", fld("begin"), instr(" = "), nested, fld("separate"), fld("end"));
    }
    let doc = read(&package(&format!("<w:p>{nested}</w:p>"))).unwrap();
    let xml = document_xml(&write(&doc).unwrap());
    let opens = xml.matches(r#"w:fldCharType="begin""#).count();
    let ends = xml.matches(r#"w:fldCharType="end""#).count();
    assert_eq!(opens, ends, "{xml}");
    // A nested field that ends after a separate and an unmatched end in the code.
    let odd = format!("<w:p>{}{}{}{}{}{}{}</w:p>", fld("begin"), instr(" IF "), fld("begin"), fld("separate"), fld("end"), fld("end"), fld("end"));
    let _ = read(&package(&odd)).map(|d| write(&d));
    // Special characters in nested codes are escaped.
    let body = format!(
        "<w:p>{}{}{}{}{}{}{}{}</w:p>",
        fld("begin"),
        instr(" IF "),
        fld("begin"),
        instr(" REF a&amp;b "),
        fld("separate"),
        t("&lt;x&gt;"),
        fld("end"),
        fld("end")
    );
    let xml = resaved(&body);
    assert!(xml.contains("REF a&amp;b"), "{xml}");
    assert!(xml.contains("&lt;x&gt;"), "{xml}");
}
