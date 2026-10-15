//! Master-page stories share the body reader/writer and the document's media store.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Header,
    HeaderLeft,
    HeaderFirst,
    Footer,
    FooterLeft,
    FooterFirst,
}

impl Slot {
    fn of(q: &str) -> Option<Self> {
        Some(match q {
            "style:header" => Self::Header,
            "style:header-left" => Self::HeaderLeft,
            "style:header-first" | "loext:header-first" => Self::HeaderFirst,
            "style:footer" => Self::Footer,
            "style:footer-left" => Self::FooterLeft,
            "style:footer-first" | "loext:footer-first" => Self::FooterFirst,
            _ => return None,
        })
    }
    fn footer(self) -> bool {
        matches!(self, Self::Footer | Self::FooterLeft | Self::FooterFirst)
    }
    fn set(self, sec: &mut SectionProps, id: u32) {
        let set = if self.footer() { &mut sec.footers } else { &mut sec.headers };
        match self {
            Self::Header | Self::Footer => set.default = Some(id),
            Self::HeaderLeft | Self::FooterLeft => set.even = Some(id),
            Self::HeaderFirst | Self::FooterFirst => {
                set.first = Some(id);
                sec.title_page = true;
            }
        }
    }
}

/// Parse each visible master-page slot once. Bound buffered events across the whole file.
pub(super) fn read(xml: &[u8], styles: &Styles, pages: &mut Pages, doc: &mut Document, fetch: &mut dyn FnMut(&str) -> Option<Vec<u8>>) {
    let empty_pages = Pages::default();
    let mut reader = Body::new(styles, &empty_pages, fetch);
    let mut master = None;
    let mut slot: Option<(Slot, usize, Vec<Ev>, bool)> = None;
    let mut seen = HashSet::new();
    let mut remaining = MAX_NOTES_TOTAL;
    events(xml, |ev| {
        if let Some((kind, depth, evs, keep)) = &mut slot {
            if matches!(&ev, Ev::End(q) if *depth == 0 && Slot::of(q) == Some(*kind)) {
                if *keep
                    && let Some(name) = &master
                    && let Some(sec) = pages.masters.get_mut(name)
                {
                    let blocks = reader.replay(evs);
                    let blocks = model::part_blocks(doc, &blocks);
                    let id = doc.add_part(if kind.footer() { PartKind::Footer } else { PartKind::Header }, blocks);
                    kind.set(sec, id);
                    doc.settings.even_odd_headers |= matches!(kind, Slot::HeaderLeft | Slot::FooterLeft);
                }
                slot = None;
                return;
            }
            match &ev {
                Ev::Start(..) => *depth += 1,
                Ev::End(_) => *depth = depth.saturating_sub(1),
                _ => {}
            }
            let bytes = match &ev {
                Ev::Start(q, a) => q.len().saturating_add(a.iter().map(|(k, v)| k.len().saturating_add(v.len())).sum::<usize>()),
                Ev::End(q) | Ev::Text(q) => q.len(),
            };
            if evs.len() >= MAX_NOTE_EVENTS || bytes > remaining {
                *keep = false;
                evs.clear();
            }
            if *keep {
                remaining -= bytes;
                evs.push(ev);
            }
            return;
        }
        match ev {
            Ev::Start(q, a) if q == "style:master-page" => {
                master = get(&a, "style:name").filter(|n| pages.masters.contains_key(*n)).map(str::to_string)
            }
            Ev::End(q) if q == "style:master-page" => master = None,
            Ev::Start(q, a) if master.is_some() => {
                if let Some(kind) = Slot::of(&q) {
                    let keep = get(&a, "style:display") != Some("false") && seen.insert((master.clone(), q));
                    slot = Some((kind, 0, Vec::new(), keep));
                }
            }
            _ => {}
        }
    });
    if let Some(first) = pages.first_name.as_ref().and_then(|n| pages.masters.get(n)) {
        pages.first = Some(first.clone());
    }
}

pub(super) fn write(writer: &mut Writer, doc: &Document, sec: &SectionProps, out: &mut String) {
    for (tag, id) in [
        ("style:header", sec.headers.default),
        ("style:header-left", sec.headers.even.filter(|_| doc.settings.even_odd_headers)),
        ("loext:header-first", sec.headers.first.filter(|_| sec.title_page)),
        ("style:footer", sec.footers.default),
        ("style:footer-left", sec.footers.even.filter(|_| doc.settings.even_odd_headers)),
        ("loext:footer-first", sec.footers.first.filter(|_| sec.title_page)),
    ] {
        if let Some(part) = id.and_then(|id| doc.parts.get(&id)) {
            out.push_str(&format!("<{tag}>"));
            writer.blocks(&model::flow_of_blocks(doc, &part.blocks), out, 0);
            writer.close_ins(out);
            out.push_str(&format!("</{tag}>"));
        }
    }
}
