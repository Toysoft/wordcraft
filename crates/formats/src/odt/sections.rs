//! Document sections: page styles for page breaks, text sections for continuous columns.
use super::*;

pub(super) fn write(w: &mut Writer, doc: &Document, flow: &Flow) -> (String, String, String) {
    let mut body = String::new();
    let mut layouts = String::new();
    let mut masters = String::from("<office:master-styles>");
    let mut start = 0;
    let mut ranges = Vec::new();
    for (i, block) in flow.blocks.iter().enumerate() {
        if let FBlock::Para(p) = block
            && let Some(sec) = p.section.as_deref()
        {
            ranges.push((start, i + 1, sec));
            start = i + 1;
        }
    }
    if start < flow.blocks.len() {
        ranges.push((start, flow.blocks.len(), &doc.last_section));
    }
    if ranges.is_empty() {
        ranges.push((0, 0, &doc.last_section));
    }
    for (i, (from, to, sec)) in ranges.into_iter().enumerate() {
        let continuous = i > 0 && sec.start == SectionStart::Continuous;
        if continuous {
            w.section_styles.push(format!(
                "<style:style style:name=\"S{i}\" style:family=\"section\"><style:section-properties>{}</style:section-properties></style:style>",
                columns(&sec.columns)
            ));
            body.push_str(&format!("<text:section text:name=\"Section{i}\" text:style-name=\"S{i}\">"));
        } else {
            let master = format!("Master{i}");
            let layout = format!("pm{i}");
            layouts.push_str(&page_layout(sec).replace("style:name=\"pm1\"", &format!("style:name=\"{layout}\"")));
            masters.push_str(&format!("<style:master-page style:name=\"{master}\" style:page-layout-name=\"{layout}\">"));
            page_parts::write(w, doc, sec, &mut masters);
            masters.push_str("</style:master-page>");
            w.next_master = Some((master, sec.page_num_start));
        }
        let blocks = flow.blocks.get(from..to).unwrap_or_default();
        w.blocks(blocks, &mut body, 0);
        w.close_ins(&mut body);
        if continuous {
            body.push_str("</text:section>");
        }
    }
    masters.push_str("</office:master-styles>");
    (body, layouts, masters)
}

pub(super) fn columns(c: &Columns) -> String {
    let sep = if c.separator { "<style:column-sep style:style=\"solid\" style:width=\"0.5pt\"/>" } else { "" };
    format!("<style:columns fo:column-count=\"{}\" fo:column-gap=\"{}\">{sep}</style:columns>", c.count.clamp(1, 45), pt(c.space))
}
