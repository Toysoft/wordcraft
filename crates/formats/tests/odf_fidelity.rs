use wordcraft_doc::{Block, Document, PartKind, RevisionKind, StoryRef};

fn fixture() -> Vec<u8> {
    br#"<office:document office:mimetype="application/vnd.oasis.opendocument.text"
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
    xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
    xmlns:dc="http://purl.org/dc/elements/1.1/">
    <office:automatic-styles>
      <style:page-layout style:name="a"><style:page-layout-properties fo:page-width="600pt" fo:page-height="800pt"/></style:page-layout>
      <style:page-layout style:name="b"><style:page-layout-properties fo:page-width="800pt" fo:page-height="600pt"/></style:page-layout>
      <style:style style:name="Wide" style:family="paragraph" style:master-page-name="B"><style:paragraph-properties style:page-number="3"/></style:style>
      <style:style style:name="Cols" style:family="section"><style:section-properties><style:columns fo:column-count="2" fo:column-gap="18pt"/></style:section-properties></style:style>
    </office:automatic-styles>
    <office:master-styles>
      <style:master-page style:name="A" style:page-layout-name="a"><style:header><text:p>Letterhead A</text:p></style:header><style:footer><text:p>Footer A</text:p></style:footer></style:master-page>
      <style:master-page style:name="B" style:page-layout-name="b"><style:header><text:p>Letterhead B</text:p></style:header></style:master-page>
    </office:master-styles>
    <office:body><office:text>
      <text:tracked-changes text:track-changes="true"><text:changed-region text:id="c"><text:insertion><office:change-info><dc:creator>Ada</dc:creator><dc:date>2026-10-14T12:00:00Z</dc:date></office:change-info></text:insertion></text:changed-region></text:tracked-changes>
      <text:p>Opening<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>Footnote body</text:p></text:note-body></text:note><office:annotation office:name="review"><dc:creator>Ben</dc:creator><text:p>Check this</text:p></office:annotation> comment range<office:annotation-end office:name="review"/><text:change-start text:change-id="c"/> added<text:change-end text:change-id="c"/></text:p>
      <text:p text:style-name="Wide">Landscape</text:p>
      <text:section text:name="Two" text:style-name="Cols"><text:p>Columns</text:p></text:section>
    </office:text></office:body></office:document>"#.to_vec()
}

fn check(d: &Document) {
    let sections = d.sections();
    assert_eq!(sections.len(), 3);
    assert_eq!(sections[0].1.page_w, 600.0);
    assert_eq!(sections[1].1.page_w, 800.0);
    assert_eq!(sections[1].1.page_num_start, Some(3));
    assert_eq!(sections[2].1.columns.count, 2);
    assert_eq!(sections[2].1.columns.space, 18.0);
    let story = |id: Option<u32>| d.plain_text(StoryRef::Part(id.unwrap()));
    assert_eq!(story(sections[0].1.headers.default), "Letterhead A");
    assert_eq!(story(sections[0].1.footers.default), "Footer A");
    assert_eq!(story(sections[1].1.headers.default), "Letterhead B");
    assert_eq!(story(sections[2].1.headers.default), "Letterhead B");
    assert!(d.parts.values().any(|p| p.kind == PartKind::Footnote));
    assert!(d.parts.iter().any(|(id, p)| p.kind == PartKind::Footnote && d.plain_text(StoryRef::Part(*id)).contains("Footnote body")));
    assert_eq!(d.comments.len(), 1);
    assert_eq!(d.comments.values().next().unwrap().author, "Ben");
    assert!(d.settings.track_changes);
    assert!(d.revisions.iter().any(|r| r.kind == RevisionKind::Insert && r.author == "Ada"));
    let Block::Para(p) = &*d.body[0] else { panic!("paragraph") };
    assert!(p.run_ranges().any(|(range, cp)| cp.ins.is_some() && p.text[range].contains("added")));
}

#[test]
fn odt_ott_and_fodt_keep_sections_stories_notes_comments_and_revisions() {
    let d = wordcraft_formats::import("fodt", &fixture()).unwrap().unwrap();
    check(&d);
    for ext in ["odt", "ott", "fodt"] {
        let bytes = wordcraft_formats::export(ext, &d).unwrap().unwrap();
        let back = wordcraft_formats::import(ext, &bytes).unwrap().unwrap();
        check(&back);
        assert_eq!(back.plain_text(StoryRef::Body), d.plain_text(StoryRef::Body), "{ext}");
    }
}

#[test]
fn note_and_comment_stories_do_not_change_the_enclosing_revision() {
    let xml = String::from_utf8(fixture())
        .unwrap()
        .replace("<text:p>Opening", "<text:p><text:change-start text:change-id=\"c\"/>Opening")
        .replace("<text:change-start text:change-id=\"c\"/> added", " added");
    let doc = wordcraft_formats::import("fodt", xml.as_bytes()).unwrap().unwrap();
    for ext in ["odt", "fodt"] {
        let bytes = wordcraft_formats::export(ext, &doc).unwrap().unwrap();
        let back = wordcraft_formats::import(ext, &bytes).unwrap().unwrap();
        let p = back.body[0].as_para().unwrap();
        for needle in ["Opening", "comment range", "added"] {
            assert!(p.run_ranges().any(|(range, cp)| cp.ins.is_some() && p.text[range].contains(needle)), "{ext}: {needle}");
        }
    }
}

#[test]
fn header_variants_and_body_images_do_not_alias_on_round_trip() {
    use std::sync::Arc;
    use wordcraft_doc::para::InlineObject;
    use wordcraft_doc::{CharProps, Paragraph, para_block};
    let mut d = Document::new();
    let picture = |name: &str| {
        let mut p = Paragraph::new();
        p.insert_object(
            0,
            InlineObject::Image { media: name.into(), w: 12.0, h: 12.0, alt: name.into(), float: Default::default(), crop: [0.0; 4], ole: None },
            &CharProps::default(),
        )
        .unwrap();
        para_block(p)
    };
    for (name, color) in [("body.png", [255, 0, 0, 255]), ("header.png", [0, 0, 255, 255])] {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(1, 1, image::Rgba(color)).write_to(&mut png, image::ImageFormat::Png).unwrap();
        d.media.insert(name.into(), Arc::new(png.into_inner()));
    }
    d.body = vec![picture("body.png")];
    let header = d.add_part(PartKind::Header, vec![picture("header.png")]);
    let first = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("First", CharProps::default()))]);
    let even = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Even", CharProps::default()))]);
    d.last_section.headers.default = Some(header);
    d.last_section.headers.first = Some(first);
    d.last_section.headers.even = Some(even);
    d.last_section.title_page = true;
    d.settings.even_odd_headers = true;
    for ext in ["odt", "ott", "fodt"] {
        let bytes = wordcraft_formats::export(ext, &d).unwrap().unwrap();
        let back = wordcraft_formats::import(ext, &bytes).unwrap().unwrap();
        let image_bytes = |blocks: &wordcraft_doc::Blocks| {
            let p = blocks[0].as_para().unwrap();
            let InlineObject::Image { media, .. } = &p.objects[0] else { panic!("image") };
            back.media[media].as_slice()
        };
        assert_eq!(image_bytes(&back.body), d.media["body.png"].as_slice(), "{ext}");
        assert_eq!(image_bytes(&back.parts[&back.last_section.headers.default.unwrap()].blocks), d.media["header.png"].as_slice());
        assert_eq!(back.plain_text(StoryRef::Part(back.last_section.headers.first.unwrap())), "First");
        assert_eq!(back.plain_text(StoryRef::Part(back.last_section.headers.even.unwrap())), "Even");
        assert!(back.last_section.title_page && back.settings.even_odd_headers);
    }
}
