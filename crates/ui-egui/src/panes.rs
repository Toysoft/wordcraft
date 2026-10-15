//! Side panes: Navigation (headings, pages, search results), Clipboard, Styles, Style Inspector,
//! Accessibility and Alt Text (see `a11y_pane`), Restrict Editing ([`crate::dialogs_review`]), Comments,
//! and the SmartArt Text Pane.

use egui::{Stroke, Ui, vec2};
use serde_json::{Value, json};
use wordcraft_doc::{Pos, StoryRef};

use crate::WordApp;
use crate::rtl::HoverText as _;
use crate::theme::{Tokens, regular, semibold};

/// The window edge a pane docks at in English. Under RTL every pane docks at the opposite edge,
/// as Word mirrors its task panes with the interface.
#[derive(Clone, Copy)]
enum Dock {
    Start,
    End,
}

/// Show a resizable pane docked at `dock`'s edge, its contents in reading order, and keep its
/// screen rect in egui memory ([`pane_rect_id`]).
fn dock(ui: &mut Ui, id: &'static str, dock: Dock, width: f32, add: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    let left = matches!(dock, Dock::Start) != crate::rtl::is_rtl();
    let panel = if left { egui::Panel::left(id) } else { egui::Panel::right(id) };
    let r = panel
        .default_size(width)
        .resizable(true)
        .frame(egui::Frame::NONE.fill(t.panel).inner_margin(10).stroke(Stroke::new(1.0, t.border)))
        .show(ui, |ui| {
            if crate::rtl::is_rtl() {
                // Rows, labels and buttons line up on the right.
                crate::rtl::column(ui, add);
            } else {
                add(ui);
            }
        });
    ui.ctx().data_mut(|d| d.insert_temp(pane_rect_id(id), r.response.rect));
}

pub fn show(app: &mut WordApp, ui: &mut Ui) {
    // SmartArt Design › Text Pane, while a SmartArt graphic WordCraft can edit is selected.
    if app.session.view.smart_art_pane && wordcraft_engine::cmd::smart_art::selected_smart_art(&app.session).is_some() {
        dock(ui, "smart_art_pane", Dock::Start, 250.0, |ui| {
            if header(ui, "Text Pane") {
                let _ = app.run("smartArt.textPane", json!({"value": false}));
                return;
            }
            crate::dialogs_smart_art::text_pane(app, ui);
        });
    }
    if app.session.view.nav_pane {
        dock(ui, "nav_pane", Dock::Start, 260.0, |ui| nav(app, ui));
    }
    if app.session.view.clipboard_pane {
        dock(ui, "clipboard_pane", Dock::Start, 250.0, |ui| clipboard(app, ui));
    }
    if app.session.view.styles_pane {
        dock(ui, "styles_pane", Dock::End, 250.0, |ui| styles(app, ui));
    }
    if app.session.view.style_inspector {
        dock(ui, "style_inspector", Dock::End, 240.0, |ui| inspector(app, ui));
    }
    if app.session.view.accessibility_pane {
        dock(ui, "accessibility_pane", Dock::End, 280.0, |ui| crate::a11y_pane::accessibility(app, ui));
    }
    if app.session.view.alt_text_pane {
        dock(ui, "alt_text_pane", Dock::End, 260.0, |ui| crate::a11y_pane::alt_text(app, ui));
    }
    if app.restrict.open {
        dock(ui, "restrict_pane", Dock::End, 270.0, |ui| {
            if header(ui, "Restrict Editing") {
                app.restrict.open = false;
                return;
            }
            crate::dialogs_review::restrict_pane(app, ui);
        });
    }
    if app.session.view.comments_pane {
        dock(ui, "comments_pane", Dock::End, 290.0, |ui| comments(app, ui));
    }
}

/// Where a pane's screen rect is kept in egui memory (for tests and the control channel).
pub(crate) fn pane_rect_id(id: &str) -> egui::Id {
    egui::Id::new(("pane_rect", id))
}

pub(crate) fn header(ui: &mut Ui, title: &str) -> bool {
    let mut close = false;
    crate::rtl::row(ui, |ui| {
        ui.label(egui::RichText::new(tl!(title)).font(semibold(15.0)));
        ui.with_layout(crate::rtl::end_layout(egui::Align::Center), |ui| {
            // Drawn, not the "✕" character: the interface fonts have no glyph for it.
            if crate::widgets::icon_button(ui, "close", tl!("Close")).clicked() {
                close = true;
            }
        });
    });
    ui.add_space(4.0);
    close
}

fn nav(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Navigation") {
        let _ = app.run("view.navigationPane", json!({"value": false}));
        return;
    }
    let qid = egui::Id::new("nav_query");
    let mut q = ui.data(|d| d.get_temp::<String>(qid)).unwrap_or_default();
    // Ctrl+F with text selected searches for it, as Word does (a selection within one line).
    let focus = std::mem::take(&mut app.nav_search_focus);
    let picked = focus.then(|| app.session.selected_text()).filter(|t| !t.is_empty() && !t.contains(['\n', '\r']) && t.chars().count() <= 255);
    if let Some(t) = &picked {
        q = t.clone();
    }
    let mut out = egui::TextEdit::singleline(&mut q).hint_text(tl!("Search document")).desired_width(f32::INFINITY).show(ui);
    let r = out.response.clone();
    if focus {
        // Focused with its text selected, so typing replaces it.
        r.request_focus();
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(q.chars().count()));
        out.state.cursor.set_char_range(Some(all));
        out.state.store(ui.ctx(), r.id);
    }
    if r.changed() || picked.is_some() {
        ui.data_mut(|d| d.insert_temp(qid, q.clone()));
        if !q.is_empty() {
            let _ = app.session.run("edit.find", &json!({"text": q}));
        } else {
            app.session.find.results.clear();
        }
    }
    ui.add_space(4.0);
    crate::rtl::row(ui, |ui| {
        for (k, l) in [("headings", "Headings"), ("pages", "Pages"), ("results", "Results")] {
            if ui.selectable_label(app.ui.nav_tab == k, tl!(l)).clicked() {
                app.ui.nav_tab = k.into();
            }
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| match app.ui.nav_tab.as_str() {
        "pages" => {
            let l = app.session.layout();
            for (i, p) in l.pages.iter().enumerate() {
                let label = crate::i18n::fmt(tl!("Page {page}"), &[("page", &p.number.to_string())]);
                if ui.selectable_label(app.session.page_hint == i, label).clicked() {
                    let _ = app.run("edit.goto", json!({"page": i + 1}));
                }
            }
        }
        "results" => {
            let results = app.session.find.results.clone();
            ui.label(egui::RichText::new(format!("{} results", results.len())).small().weak());
            for (a, b) in results.iter().take(500) {
                let Some(p) = app.session.doc.para_at(a) else { continue };
                let from = p.clamp(a.off.saturating_sub(30));
                let to = p.clamp((b.off + 40).min(p.len()));
                let pre = p.text.get(from..a.off).unwrap_or("");
                let hit = p.text.get(a.off..b.off).unwrap_or("");
                let post = p.text.get(b.off..to).unwrap_or("");
                let mut job = egui::text::LayoutJob::default();
                let plain = egui::TextFormat { font_id: regular(12.0), color: ui.visuals().text_color(), ..Default::default() };
                // The match is marked the way Find marks it on the page, per theme (#656).
                let mark = Tokens::get(ui.ctx()).find_highlight;
                let marked = egui::TextFormat { font_id: semibold(12.0), background: mark, color: ui.visuals().text_color(), ..Default::default() };
                let snippet = p.text.get(from..to).unwrap_or("");
                if !crate::bidi::has_rtl(snippet) {
                    job.append(pre, 0.0, plain.clone());
                    job.append(hit, 0.0, marked);
                    job.append(post, 0.0, plain);
                } else {
                    let (hit_from, hit_to) = (a.off.saturating_sub(from), b.off.saturating_sub(from));
                    let measure = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), semibold(12.0), egui::Color32::WHITE).size().x);
                    // The paragraph's direction as the page lays it out (its style's too).
                    let base_rtl = app.session.doc.styles.resolve_para(&p.props).bidi;
                    bidi_snippet(&mut job, snippet, base_rtl, (hit_from, hit_to), ui.available_width(), measure, &plain, &marked);
                }
                job.wrap.max_width = ui.available_width();
                if ui.add(egui::Button::new(job).frame(false)).clicked() {
                    let _ = app.run("select.range", json!({"anchor": a, "focus": b}));
                }
                ui.separator();
            }
        }
        _ => {
            let caret = app.session.sel.focus.clone();
            let mut current: Option<usize> = None;
            let mut items = Vec::new();
            for path in app.session.doc.para_paths(StoryRef::Body) {
                let Some(p) = app.session.doc.para(StoryRef::Body, &path) else { continue };
                let rp = app.session.doc.styles.resolve_para(&p.props);
                if let Some(lv) = rp.outline_level {
                    let txt = p.plain_text().trim().to_string();
                    if txt.is_empty() || rp.style.starts_with("TOC") {
                        continue;
                    }
                    if path <= caret.path {
                        current = Some(items.len());
                    }
                    items.push((path, lv, txt));
                }
            }
            if items.is_empty() {
                crate::rtl::wrapped_label(ui, tl!("Create an interactive outline of your document.\n\nIt's a great way to keep track of where you are or quickly move your content around.\n\nTo get started, go to the Home tab and apply Heading styles to the headings in your document."), |s| egui::RichText::new(s).weak());
            }
            for (i, (path, lv, txt)) in items.into_iter().enumerate() {
                if nav_heading(ui, lv, &txt, current == Some(i)).clicked() {
                    let _ = app.run("caret.set", json!({"pos": Pos { story: StoryRef::Body, path, off: 0 }}));
                    // Align the chosen heading's page immediately against this pane, rather
                    // than the indeterminate minimum movement from scroll_to_rect.
                    app.canvas.nav_to_caret = true;
                    app.canvas.scroll_to_caret = false;
                    app.canvas.want_focus = true;
                }
            }
        }
    });
}

/// A find result's line of context with right-to-left text, appended to `job` in draw order:
/// broken into lines in logical order (each at most `width`), each line reordered as a whole
/// (`base_rtl` is the paragraph's direction) and cut where the hit (`hit`, byte offsets in
/// `snippet`) starts and ends, so the hit keeps its mark wherever it lands.
#[allow(clippy::too_many_arguments)]
fn bidi_snippet(
    job: &mut egui::text::LayoutJob,
    snippet: &str,
    base_rtl: bool,
    hit: (usize, usize),
    width: f32,
    measure: impl Fn(&str) -> f32,
    plain: &egui::TextFormat,
    marked: &egui::TextFormat,
) {
    let lines = crate::bidi::line_ranges(snippet, width, measure);
    for (n, line) in lines.iter().enumerate() {
        if n > 0 {
            job.append("\n", 0.0, plain.clone());
        }
        let text = snippet.get(line.clone()).unwrap_or_default();
        for (piece, mirror) in crate::bidi::visual_pieces(text, base_rtl) {
            // Offsets of the piece in `snippet`, cut at the hit's two ends.
            let (start, end) = (line.start.saturating_add(piece.start), line.start.saturating_add(piece.end));
            let mut cuts = vec![start, end];
            cuts.extend([hit.0, hit.1].into_iter().filter(|c| *c > start && *c < end));
            cuts.sort_unstable();
            for w in cuts.windows(2) {
                let (Some(&x), Some(&y)) = (w.first(), w.get(1)) else { continue };
                let Some(part) = snippet.get(x..y) else { continue };
                let format = if x >= hit.0 && y <= hit.1 && hit.0 < hit.1 { marked } else { plain };
                job.append(&crate::bidi::piece_text(part, mirror), 0.0, format.clone());
            }
        }
    }
}

/// Constrain each heading to the available pane width so long document text cannot
/// enlarge the panel. Hovering shows the heading's first [`HEADING_SHOWN_CHARS`] characters
/// (the whole heading when shorter). Headings draw in bidi order (an Arabic heading reads right
/// to left in any interface language).
///
/// Only the first [`HEADING_SHOWN_CHARS`] characters are drawn or measured (a heading can be a
/// whole 64 KiB paragraph), and the hover text is built only while hovered.
fn nav_heading(ui: &mut Ui, level: u8, text: &str, selected: bool) -> egui::Response {
    let font = if level == 0 { semibold(12.5) } else { regular(12.5) };
    let text = shown_prefix(text, HEADING_SHOWN_CHARS);
    crate::rtl::row(ui, |ui| {
        ui.add_space(level as f32 * 14.0);
        let room = ui.available_width() - 2.0 * ui.spacing().button_padding.x;
        let shown = fit_user_text(ui, text, &font, room);
        let label = egui::RichText::new(shown).font(font);
        // The grow atom takes the spare width, so the text starts flush at the start edge under
        // its parent instead of centred, as in Word (#1199): after the text, or before it under RTL.
        let atoms = if crate::rtl::is_rtl() { (egui::Atom::grow(), label.into()) } else { (label.into(), egui::Atom::grow()) };
        ui.add_sized(vec2(ui.available_width(), ui.spacing().interact_size.y), egui::Button::selectable(selected, atoms).truncate()).on_hover_ui(
            |ui| {
                // As `Response::on_hover_text`, but the text is converted only while hovered.
                ui.set_max_width(ui.spacing().tooltip_width);
                ui.add(egui::Label::new(crate::i18n::user_text(text).into_owned()));
            },
        )
    })
    .inner
}

/// The most characters of a heading the Navigation pane draws, measures or shows on hover: far
/// more than fit in the pane, few enough that reordering and measuring it every frame is cheap.
const HEADING_SHOWN_CHARS: usize = 512;

/// The first `max` characters of `text` (all of it when shorter), cut at a character boundary.
fn shown_prefix(text: &str, max: usize) -> &str {
    text.char_indices().nth(max).and_then(|(i, _)| text.get(..i)).unwrap_or(text)
}

/// Document text for one line `width` wide, in draw order. Left-to-right text is returned as it is
/// (egui truncates it at its end). Text that needs reordering is cut in logical order, with "…"
/// at its logical end, before it is converted: egui would cut the drawn string at its right,
/// which is the start of a right-to-left text.
fn fit_user_text<'a>(ui: &Ui, text: &'a str, font: &egui::FontId, width: f32) -> std::borrow::Cow<'a, str> {
    let shown = crate::i18n::user_text(text);
    if matches!(shown, std::borrow::Cow::Borrowed(_)) {
        return shown;
    }
    let measure = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE).size().x);
    if measure(&shown) <= width {
        return shown;
    }
    let rtl = crate::bidi::first_strong_is_rtl(text);
    let ends: Vec<usize> = text.char_indices().map(|(i, _)| i).skip(1).collect();
    let cut = |n: usize| {
        crate::bidi::visual(&format!("{}…", text.get(..ends.get(n).copied().unwrap_or(text.len())).unwrap_or_default().trim_end()), rtl).into_owned()
    };
    // The longest prefix that fits, by bisection over its length in characters (bounded by the text).
    let (mut lo, mut hi) = (0usize, ends.len());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if measure(&cut(mid)) <= width { lo = mid } else { hi = mid - 1 }
    }
    std::borrow::Cow::Owned(cut(lo))
}

/// Items collected by Copy and Cut, newest first; clicking one pastes it at the caret.
fn clipboard(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Clipboard") {
        let _ = app.run("edit.clipboardPane", json!({"value": false}));
        return;
    }
    let items: Vec<String> = app.session.clip_history.items().iter().map(|i| i.preview()).collect();
    crate::rtl::row(ui, |ui| {
        if ui.add_enabled(!items.is_empty(), egui::Button::new(tl!("Paste All"))).clicked() {
            let _ = app.run("edit.pasteAllClipboard", json!({}));
            app.canvas.want_focus = true;
        }
        if ui.add_enabled(!items.is_empty(), egui::Button::new(tl!("Clear All"))).clicked() {
            let _ = app.run("edit.clearClipboard", json!({}));
        }
    });
    ui.add_space(4.0);
    let hint = if items.is_empty() { "Nothing collected yet. Items you copy or cut appear here." } else { "Click an item to paste it." };
    crate::rtl::wrapped_label(ui, tl!(hint), |s| egui::RichText::new(s).small().weak());
    ui.separator();
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, preview) in items.iter().enumerate() {
            egui::Frame::NONE.fill(t.input).stroke(Stroke::new(1.0, t.border)).corner_radius(6).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let text = if preview.is_empty() { tl!("(picture or object)").to_string() } else { preview.clone() };
                let mut job =
                    egui::text::LayoutJob::single_section(text, egui::TextFormat { font_id: regular(12.5), color: t.text, ..Default::default() });
                job.wrap.max_width = ui.available_width();
                job.wrap.max_rows = 3;
                if ui.add(egui::Button::new(job).frame(false)).rtl_hover_text(tl!("Paste")).clicked() {
                    let _ = app.run("edit.pasteClipboardItem", json!({"index": i}));
                    app.canvas.want_focus = true;
                }
                ui.with_layout(crate::rtl::end_layout(egui::Align::Min), |ui| {
                    if ui.small_button(tl!("Delete")).clicked() {
                        let _ = app.run("edit.deleteClipboardItem", json!({"index": i}));
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
}

fn styles(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Styles") {
        let _ = app.run("view.stylesPane", json!({"value": false}));
        return;
    }
    let st = app.session.run("format.state", &json!({})).unwrap_or_default();
    let cur = st.get("style").and_then(Value::as_str).unwrap_or("Normal").to_string();
    let current_name = app
        .session
        .doc
        .styles
        .get(&cur)
        .map(crate::i18n::style_name)
        .unwrap_or_else(|| crate::i18n::user_text(st.get("styleName").and_then(Value::as_str).unwrap_or("")).into_owned());
    // `style_name` gives draw order for built-in and author's names alike.
    ui.label(egui::RichText::new(crate::i18n::fmt_translated(tl!("Current style: {style}"), &[("style", &current_name)])).small());
    crate::rtl::row(ui, |ui| {
        if ui.button(tl!("New Style…")).clicked() {
            app.dialog = crate::dialogs::Dialog::open("newStyle", app);
        }
        if ui.button(tl!("Update to Match")).clicked() {
            let _ = app.run("styles.updateToMatch", json!({}));
        }
        if ui.button(tl!("Clear")).clicked() {
            let _ = app.run("format.clear", json!({}));
        }
    });
    if ui.button(tl!("Manage Styles…")).clicked() {
        let _ = app.run("styles.manage", json!({}));
    }
    if ui.selectable_label(app.session.view.style_inspector, tl!("Style Inspector")).clicked() {
        let _ = app.run("styles.inspector", json!({}));
    }
    ui.separator();
    // Sorted by the names in reading order; only the shown name is in draw order.
    let mut list: Vec<(String, String, String, bool)> = app
        .session
        .doc
        .styles
        .styles
        .iter()
        .filter(|s| !s.hidden && s.kind != wordcraft_doc::StyleKind::Table)
        .map(|s| {
            let key = crate::i18n::lowercase(&crate::i18n::style_name_logical(s));
            (key, s.id.clone(), crate::i18n::style_name(s), s.kind == wordcraft_doc::StyleKind::Character)
        })
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (_, id, name, is_char) in list {
            crate::rtl::row(ui, |ui| {
                let r = ui.selectable_label(id == cur, &name);
                ui.with_layout(crate::rtl::end_layout(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(if is_char { "a" } else { "¶" }).weak());
                });
                if r.clicked() {
                    let _ = app.run("para.style", json!({"style": id}));
                    app.canvas.want_focus = true;
                }
                crate::rtl::on_context_menu(&r, |ui| {
                    if ui.button(tl!("Modify…")).clicked() {
                        app.dialog = crate::dialogs::Dialog::modify_style(app, &id);
                        ui.close();
                    }
                    if ui.button(tl!("Update to Match Selection")).clicked() {
                        let _ = app.run("styles.updateToMatch", json!({"style": id}));
                        ui.close();
                    }
                    if ui.button(tl!("Delete")).clicked() {
                        let _ = app.run("styles.delete", json!({"style": id}));
                        ui.close();
                    }
                });
            });
        }
    });
}

fn comments(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Comments") {
        let _ = app.run("view.commentsPane", json!({"value": false}));
        return;
    }
    if crate::widgets::icon_text_button(ui, "newComment", tl!("New Comment"), regular(12.5)).clicked() {
        let _ = app.run("review.newComment", json!({"text": ""}));
    }
    ui.separator();
    let list = app.session.run("review.comments", &json!({})).unwrap_or_default();
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().show(ui, |ui| {
        for c in list.as_array().cloned().unwrap_or_default() {
            let id = c.get("id").and_then(Value::as_u64).unwrap_or(0);
            let reply = c.get("parent").is_some_and(|p| !p.is_null());
            egui::Frame::NONE.fill(t.input).stroke(Stroke::new(1.0, t.border)).corner_radius(6).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if reply {
                    ui.add_space(0.0);
                }
                crate::rtl::row(ui, |ui| {
                    ui.label(egui::RichText::new(crate::i18n::user_text(c.get("author").and_then(Value::as_str).unwrap_or(""))).font(semibold(12.0)));
                    ui.label(egui::RichText::new(comment_stamp(c.get("date").and_then(Value::as_str).unwrap_or(""))).small().weak());
                    if c.get("resolved").and_then(Value::as_bool).unwrap_or(false) {
                        ui.label(egui::RichText::new(tl!("Resolved")).small().color(t.green));
                    }
                });
                // Editable comment text (one undo step per edit, like the balloons).
                comment_editor(app, ui, id as u32, None, false);
                let open_key = egui::Id::new(("comment_reply_open", id));
                let mut replying = ui.data(|d| d.get_temp::<bool>(open_key)).unwrap_or(false);
                crate::rtl::row(ui, |ui| {
                    if ui.small_button(tl!("Go to")).clicked()
                        && let Some(a) = c.get("anchor").filter(|a| !a.is_null())
                    {
                        let _ = app.run("caret.set", json!({"pos": a}));
                    }
                    if ui.small_button(tl!("Reply")).clicked() {
                        replying = true;
                        ui.data_mut(|d| d.insert_temp(open_key, true));
                        ui.data_mut(|d| d.insert_temp(egui::Id::new(("comment_reply_focus", id)), true));
                    }
                    let resolved = c.get("resolved").and_then(Value::as_bool).unwrap_or(false);
                    if ui.small_button(if resolved { tl!("Reopen") } else { tl!("Resolve") }).clicked() {
                        let _ = app.run("review.resolveComment", json!({"id": id}));
                    }
                    if ui.small_button(tl!("Delete")).clicked() {
                        let _ = app.run("review.deleteComment", json!({"id": id}));
                    }
                });
                if replying {
                    let focus = ui.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(("comment_reply_focus", id)))).unwrap_or(false);
                    if reply_editor(app, ui, id as u32, None, focus) {
                        ui.data_mut(|d| d.remove::<bool>(open_key));
                    }
                }
            });
            ui.add_space(6.0);
        }
    });
}

fn edit_key(id: u32) -> egui::Id {
    egui::Id::new(("comment_edit", id))
}

fn reply_key(id: u32) -> egui::Id {
    egui::Id::new(("comment_reply", id))
}

/// A comment's text, editable in place (the Comments pane and the balloons). Typing stays in a
/// draft until the field loses focus; then [`commit_comment`] writes it back with one
/// `review.editComment`, so each editing session is one undo step.
pub(crate) fn comment_editor(app: &mut WordApp, ui: &mut Ui, id: u32, font: Option<egui::FontId>, focus: bool) {
    let Some(part) = app.session.doc.comments.get(&id).map(|c| c.part) else { return };
    let key = edit_key(id);
    let open_key = egui::Id::new(("comment_edit_open", id));
    let draft = ui.data(|d| d.get_temp::<String>(key));
    let opened = ui.data(|d| d.get_temp::<bool>(open_key)).unwrap_or(false);
    let mut text = draft.clone().unwrap_or_else(|| app.session.doc.plain_text(StoryRef::Part(part)));
    // egui's text field draws its text in logical order, so right-to-left text shows in bidi
    // order while it isn't being edited, and becomes the field when clicked.
    if draft.is_none() && !focus && !opened && crate::bidi::has_rtl(&text) && text.len() <= MAX_SHOWN_COMMENT {
        let font = font.unwrap_or_else(|| egui::TextStyle::Body.resolve(ui.style()));
        let r = user_text_block(ui, &text, &font).on_hover_cursor(egui::CursorIcon::Text);
        if r.clicked() {
            // Hold the keyboard until the field takes it next frame, so the click doesn't count as
            // one on empty chrome (which hands focus back to the page).
            r.request_focus();
            ui.data_mut(|d| d.insert_temp(open_key, true));
            ui.data_mut(|d| d.insert_temp(egui::Id::new(("comment_edit_focus", id)), true));
            ui.ctx().request_repaint();
        }
        return;
    }
    let mut edit = egui::TextEdit::multiline(&mut text).desired_rows(1).desired_width(f32::INFINITY).hint_text(tl!("Add a comment"));
    if let Some(f) = font {
        edit = edit.font(f);
    }
    let r = ui.add(edit);
    let clicked_open = opened && ui.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(("comment_edit_focus", id)))).unwrap_or(false);
    if focus || clicked_open {
        r.request_focus();
    }
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(key, text));
    }
    if r.lost_focus() {
        ui.data_mut(|d| d.remove::<bool>(open_key));
        commit_comment(app, ui.ctx(), id);
    }
}

/// Longest comment text drawn in bidi order (longer ones show in the text field as they are).
const MAX_SHOWN_COMMENT: usize = 8 * 1024;

/// Document text of several paragraphs, each wrapped to the width in logical order and drawn in
/// bidi order in its own direction (right-to-left paragraphs at the right). Clickable.
fn user_text_block(ui: &mut Ui, text: &str, font: &egui::FontId) -> egui::Response {
    let width = ui.available_width();
    let color = ui.visuals().text_color();
    let measure = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), color).size().x);
    let paragraphs: Vec<(bool, Vec<String>)> = text
        .split('\n')
        .map(|para| {
            let rtl = crate::bidi::first_strong_is_rtl(para);
            (rtl, crate::bidi::wrap(para, rtl, width, measure))
        })
        .collect();
    ui.scope(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        for (rtl, lines) in paragraphs {
            let align = if rtl { egui::Align::Max } else { egui::Align::Min };
            ui.with_layout(egui::Layout::top_down(align), |ui| {
                for line in lines {
                    ui.add(egui::Label::new(egui::RichText::new(line).font(font.clone())).wrap_mode(egui::TextWrapMode::Extend).selectable(false));
                }
            });
        }
    })
    .response
    .interact(egui::Sense::click())
}

/// Write a comment's draft text back, when it has one that differs from the comment.
pub(crate) fn commit_comment(app: &mut WordApp, ctx: &egui::Context, id: u32) {
    let Some(text) = ctx.data_mut(|d| d.remove_temp::<String>(edit_key(id))) else { return };
    let Some(part) = app.session.doc.comments.get(&id).map(|c| c.part) else { return };
    if text != app.session.doc.plain_text(StoryRef::Part(part)) {
        let _ = app.run("review.editComment", json!({"id": id, "text": text}));
    }
}

/// The comment a reply to `id` answers: replies to a reply join its thread.
fn thread_root(app: &WordApp, id: u32) -> u32 {
    match app.session.doc.comments.get(&id).and_then(|c| c.parent) {
        Some(p) if app.session.doc.comments.contains_key(&p) => p,
        _ => id,
    }
}

/// A field for replying to a comment. When it loses focus a non-empty reply is posted with
/// `review.reply`; returns true when the field is done (posted or left empty).
pub(crate) fn reply_editor(app: &mut WordApp, ui: &mut Ui, id: u32, font: Option<egui::FontId>, focus: bool) -> bool {
    let key = reply_key(id);
    let mut text = ui.data(|d| d.get_temp::<String>(key)).unwrap_or_default();
    let mut edit = egui::TextEdit::multiline(&mut text).desired_rows(1).desired_width(f32::INFINITY).hint_text(tl!("Write a reply"));
    if let Some(f) = font {
        edit = edit.font(f);
    }
    let r = ui.add(edit);
    if focus {
        r.request_focus();
    }
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(key, text));
    }
    if r.lost_focus() {
        commit_reply(app, ui.ctx(), id);
        return true;
    }
    false
}

/// Post a pending reply draft to comment `id` (nothing when there is none or it is blank).
pub(crate) fn commit_reply(app: &mut WordApp, ctx: &egui::Context, id: u32) {
    let Some(text) = ctx.data_mut(|d| d.remove_temp::<String>(reply_key(id))) else { return };
    if !text.trim().is_empty() {
        let root = thread_root(app, id);
        let _ = app.run("review.reply", json!({"id": root, "text": text.trim_end()}));
    }
}

/// Style Inspector: the paragraph and character levels at the caret, each with its style and the
/// direct formatting on top, and a button to clear each level.
fn inspector(app: &mut WordApp, ui: &mut Ui) {
    if header(ui, "Style Inspector") {
        let _ = app.run("styles.inspector", json!({"value": false}));
        return;
    }
    // Read-only, so call it directly: running the command every frame would pay for the command
    // machinery and consume per-command state (a pending `join_next_undo`) just to draw the pane.
    let r = wordcraft_engine::cmd::inspector::inspect(&app.session);
    let t = Tokens::get(ui.ctx());
    let none = tl!("None").to_string();
    let para = &r["paragraph"];
    let chr = &r["character"];
    let pstyle = para["styleName"].as_str().unwrap_or("Normal").to_string();
    let cstyle = chr["styleName"].as_str().map(str::to_string);
    // Built-in style names show in the interface language; the document keeps the English ones.
    let shown = |id: &Value, name: &str| {
        id.as_str()
            .and_then(|id| app.session.doc.styles.get(id))
            .map(crate::i18n::style_name)
            .unwrap_or_else(|| crate::i18n::user_text(name).into_owned())
    };
    let pstyle_shown = shown(&para["style"], &pstyle);
    let cstyle_shown = cstyle.as_deref().map(|n| shown(&chr["style"], n));
    let pdirect = direct_list(&para["direct"]);
    let cdirect = direct_list(&chr["direct"]);
    let mut clear: Option<&str> = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        section(ui, &t, "Paragraph", |ui| {
            if level_row(ui, &t, "Style", &pstyle_shown, false, "Reset to Normal", pstyle == "Normal" && para["style"] == "Normal") {
                clear = Some("paragraphStyle");
            }
            let (txt, empty) = if pdirect.is_empty() { (none.clone(), true) } else { (pdirect.join(", "), false) };
            if level_row(ui, &t, "Direct formatting", &txt, empty, "Clear Paragraph Formatting", empty) {
                clear = Some("paragraphFormatting");
            }
        });
        ui.add_space(8.0);
        section(ui, &t, "Characters", |ui| {
            let (txt, empty) = match &cstyle_shown {
                Some(n) => (n.clone(), false),
                None => (none.clone(), true),
            };
            if level_row(ui, &t, "Style", &txt, empty, "Clear Character Style", empty) {
                clear = Some("characterStyle");
            }
            let (txt, empty) = if cdirect.is_empty() { (none.clone(), true) } else { (cdirect.join(", "), false) };
            if level_row(ui, &t, "Direct formatting", &txt, empty, "Clear Character Formatting", empty) {
                clear = Some("characterFormatting");
            }
        });
        ui.add_space(10.0);
        crate::rtl::row(ui, |ui| {
            if ui.button(tl!("Clear All")).rtl_hover_text(tl!("Clear All Formatting")).clicked() {
                let _ = app.run("format.clear", json!({}));
                app.canvas.want_focus = true;
            }
            if ui.button(tl!("Styles Pane")).clicked() {
                let _ = app.run("view.stylesPane", json!({"value": true}));
            }
        });
    });
    if let Some(level) = clear {
        let _ = app.run("styles.inspectorClear", json!({"level": level}));
        app.canvas.want_focus = true;
    }
}

/// A titled, bordered group in the inspector.
fn section(ui: &mut Ui, t: &Tokens, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.label(egui::RichText::new(tl!(title)).font(semibold(12.5)).color(t.text));
    ui.add_space(2.0);
    egui::Frame::NONE.fill(t.input).stroke(Stroke::new(1.0, t.border)).corner_radius(4).inner_margin(8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

/// One level: a caption, its value, and a clear button (disabled when there's nothing to clear).
/// Returns true when the button was clicked.
fn level_row(ui: &mut Ui, t: &Tokens, caption: &str, value: &str, dim: bool, tip: &str, disabled: bool) -> bool {
    ui.label(egui::RichText::new(tl!(caption)).font(regular(11.0)).color(t.text_dim));
    let mut clicked = false;
    crate::rtl::row(ui, |ui| {
        let w = (ui.available_width() - 26.0).max(40.0);
        ui.allocate_ui(vec2(w, 18.0), |ui| {
            ui.set_width(w);
            let text = egui::RichText::new(value).font(regular(12.5)).color(if dim { t.text_disabled } else { t.text });
            ui.add(egui::Label::new(text).wrap());
        });
        ui.with_layout(crate::rtl::end_layout(egui::Align::Min), |ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), egui::Sense::click());
            let enabled = !disabled;
            if enabled && resp.hovered() {
                ui.painter().rect_filled(r, 3.0, t.hover);
            }
            let c = if enabled { t.icon } else { t.text_disabled };
            crate::icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(r.center(), vec2(16.0, 16.0)),
                "clear",
                c,
                if enabled { t.accent } else { c },
            );
            let resp = resp.rtl_hover_text(tl!(tip));
            clicked = enabled && resp.clicked();
        });
    });
    ui.add_space(4.0);
    clicked
}

/// Readable descriptions of the inspector's `direct` entries (`[{"prop", "value"}]`).
fn direct_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().take(64).map(|d| describe(d["prop"].as_str().unwrap_or(""), &d["value"])).collect()).unwrap_or_default()
}

/// Each `{"prop", "value"}` of a list (the Style Inspector's, or a formatting change's) as text.
pub(crate) fn describe_props(list: &[Value]) -> Vec<String> {
    list.iter().take(64).map(|d| describe(d["prop"].as_str().unwrap_or(""), &d["value"])).collect()
}

/// "Bold", "Not Italic", "Font: Arial", "Size: 14 pt"…
fn describe(prop: &str, v: &Value) -> String {
    let label = match prop {
        "font" => "Font",
        "size" => "Size",
        "bold" => "Bold",
        "italic" => "Italic",
        "underline" => "Underline",
        "underlineColor" => "Underline color",
        "strike" => "Strikethrough",
        "doubleStrike" => "Double strikethrough",
        "color" => "Font color",
        "highlight" => "Highlight",
        "shading" => "Shading",
        "vertAlign" => "Position",
        "caps" => "All caps",
        "smallCaps" => "Small caps",
        "hidden" => "Hidden",
        "spacing" => "Character spacing",
        "scale" => "Scale",
        "position" => "Raised/lowered",
        "outline" => "Outline",
        "shadow" => "Shadow",
        "emboss" => "Emboss",
        "engrave" => "Engrave",
        "align" => "Alignment",
        "indentLeft" => "Left indent",
        "indentRight" => "Right indent",
        "indentFirst" => "First line",
        "spaceBefore" => "Space before",
        "spaceAfter" => "Space after",
        "lineSpacing" => "Line spacing",
        "contextualSpacing" => "Don't add space between paragraphs of the same style",
        "mirrorIndents" => "Mirror indents",
        "keepNext" => "Keep with next",
        "keepLines" => "Keep lines together",
        "pageBreakBefore" => "Page break before",
        "widowControl" => "Widow/Orphan control",
        "outlineLevel" => "Outline level",
        "tabs" => "Tabs",
        "borders" => "Borders",
        "suppressHyphens" => "Don't hyphenate",
        "suppressLineNumbers" => "Suppress line numbers",
        "bidi" => "Right-to-left",
        "dropCap" => "Drop cap",
        "style" => "Style",
        "numbering" => "Numbering",
        "lang" => "Language",
        other => other,
    };
    let label = tl!(label);
    let pt = |x: f64| crate::i18n::fmt(tl!("{n} pt"), &[("n", &trim_num(x))]);
    let hex = |c: &Value| -> Option<String> {
        let a = c.as_array()?;
        let ch = |i: usize| a.get(i).and_then(Value::as_u64).unwrap_or(0).min(255);
        Some(format!("#{:02X}{:02X}{:02X}", ch(0), ch(1), ch(2)))
    };
    let value = match (prop, v) {
        (_, Value::Bool(true)) => return label.to_string(),
        (_, Value::Bool(false)) => return crate::i18n::fmt_translated(tl!("Not {name}"), &[("name", label)]),
        ("scale", Value::Number(n)) => format!("{}%", trim_num(n.as_f64().unwrap_or(100.0))),
        ("outlineLevel", Value::Number(n)) => match n.as_u64().unwrap_or(9) {
            l @ 0..=8 => crate::i18n::fmt(tl!("Level {n}"), &[("n", &(l + 1).to_string())]),
            _ => tl!("Body Text").to_string(),
        },
        ("dropCap", Value::Number(n)) => crate::i18n::fmt(tl!("{n} lines"), &[("n", &n.to_string())]),
        (_, Value::Number(n)) => pt(n.as_f64().unwrap_or(0.0)),
        ("align", Value::String(s)) => tl!(match s.as_str() {
            "center" => "Centered",
            "right" => "Right",
            "justify" => "Justified",
            "distribute" => "Distributed",
            _ => "Left",
        })
        .to_string(),
        ("color", Value::String(_)) => tl!("Automatic").to_string(),
        ("color", Value::Object(o)) => o.get("Rgb").and_then(hex).unwrap_or_default(),
        (_, Value::Array(_)) if prop != "tabs" => hex(v).unwrap_or_default(),
        ("lineSpacing", Value::Object(o)) => {
            let x = o.get("value").and_then(Value::as_f64).unwrap_or(1.0);
            match o.get("rule").and_then(Value::as_str) {
                Some("atLeast") => crate::i18n::fmt_translated(tl!("At least {n}"), &[("n", &pt(x))]),
                Some("exactly") => crate::i18n::fmt_translated(tl!("Exactly {n}"), &[("n", &pt(x))]),
                _ => crate::i18n::fmt(tl!("{n} lines"), &[("n", &trim_num(x))]),
            }
        }
        (_, Value::String(s)) => tl!(&camel_words(s)).to_string(),
        _ => return label.to_string(),
    };
    // Both parts are catalog text or numbers: draw order already.
    crate::i18n::labeled(label, &value)
}

/// `1.50` → `1.5`, `12.0` → `12`.
fn trim_num(x: f64) -> String {
    let s = format!("{:.2}", if x.is_finite() { x } else { 0.0 });
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `2026-10-12T04:44:00Z` → `2026-10-12 04:44` (date and time, no seconds) for comment cards.
fn comment_stamp(iso: &str) -> String {
    iso.chars().take(16).map(|c| if c == 'T' { ' ' } else { c }).collect()
}

/// `dotDash` → `Dot dash` (enum values from the document model).
fn camel_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().take(64).enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::comment_stamp;

    #[test]
    fn comment_stamp_keeps_date_and_time() {
        assert_eq!(comment_stamp("2026-10-12T04:44:31Z"), "2026-10-12 04:44");
        assert_eq!(comment_stamp("2026-10-12"), "2026-10-12");
        assert_eq!(comment_stamp(""), "");
    }
}

#[cfg(test)]
mod rtl_tests {
    use egui_kittest::kittest::Queryable as _;
    use serde_json::json;

    use crate::{Services, WordApp};

    /// The sample document with `panes` turned on, in `lang`, after a few frames: each pane's
    /// header label rect (found by its translated title) and the canvas rect.
    fn pane_rects(lang: &str, panes: &[(&str, &str)]) -> (Vec<egui::Rect>, egui::Rect) {
        let mut a = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::sample::sample_document()), Services::default());
        a.run("ui.language", json!({"value": lang})).unwrap();
        for (cmd, _) in panes {
            a.run(cmd, json!({"value": true})).unwrap();
        }
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(5);
        let canvas = h.state().canvas.canvas_rect.unwrap();
        let l = crate::i18n::Lang::from_code(lang).unwrap();
        // The pane's header is the lowest node with its title (the ribbon may use the same word).
        let rects = panes
            .iter()
            .map(|(_, title)| {
                let label = crate::i18n::tr(l, title);
                h.query_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.min.y.total_cmp(&b.min.y)).unwrap()
            })
            .collect();
        crate::i18n::set_current(crate::i18n::Lang::EN);
        (rects, canvas)
    }

    /// Word mirrors its task panes with the interface: the panes that dock at the right in English
    /// (Styles, Accessibility, Comments) dock at the left in Arabic.
    #[test]
    fn task_panes_dock_on_the_left_in_arabic() {
        for pane in [("view.stylesPane", "Styles"), ("view.accessibilityPane", "Accessibility"), ("view.commentsPane", "Comments")] {
            let (en, canvas) = pane_rects("en", &[pane]);
            assert!(en[0].left() > canvas.right() - 1.0, "english: {} ({:?}) is right of the canvas ({canvas:?})", pane.1, en[0]);
            let (ar, canvas) = pane_rects("ar", &[pane]);
            assert!(ar[0].right() < canvas.left() + 1.0, "arabic: {} ({:?}) is left of the canvas ({canvas:?})", pane.1, ar[0]);
        }
    }

    /// The Navigation pane docks at the left in English, so it moves to the right in Arabic (the
    /// mirror of Word's layout, as Arabic Word has it).
    #[test]
    fn navigation_pane_docks_on_the_right_in_arabic() {
        let nav = ("view.navigationPane", "Navigation");
        let (en, canvas) = pane_rects("en", &[nav]);
        assert!(en[0].right() < canvas.left() + 1.0, "english: Navigation ({:?}) is left of the canvas ({canvas:?})", en[0]);
        let (ar, canvas) = pane_rects("ar", &[nav]);
        assert!(ar[0].left() > canvas.right() - 1.0, "arabic: Navigation ({:?}) is right of the canvas ({canvas:?})", ar[0]);
    }

    /// A pane's header reads from the right in Arabic: its title sits at the pane's right edge.
    #[test]
    fn pane_header_title_starts_at_the_right_in_arabic() {
        for lang in ["en", "ar"] {
            let mut a = WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::sample::sample_document()), Services::default());
            a.run("ui.language", json!({"value": lang})).unwrap();
            a.run("view.stylesPane", json!({"value": true})).unwrap();
            let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
                |ui, app: &mut WordApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                a,
            );
            h.run_steps(5);
            let pane = h.ctx.data(|d| d.get_temp::<egui::Rect>(super::pane_rect_id("styles_pane"))).unwrap();
            let label = crate::i18n::tr(crate::i18n::Lang::from_code(lang).unwrap(), "Styles");
            let title = h.query_all_by_label(label).map(|n| n.rect()).max_by(|a, b| a.min.y.total_cmp(&b.min.y)).unwrap();
            crate::i18n::set_current(crate::i18n::Lang::EN);
            if lang == "ar" {
                assert!(pane.right() - title.right() < 20.0, "arabic: the title ({title:?}) starts at the pane's right edge ({pane:?})");
            } else {
                assert!(title.left() - pane.left() < 20.0, "english: the title ({title:?}) starts at the pane's left edge ({pane:?})");
            }
        }
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;

    #[test]
    fn long_headings_truncate_without_stretching_navigation() {
        let text = "An extremely long heading with words that must stay inside the navigation pane ".repeat(20);
        for (width, level) in [(260.0, 0), (180.0, 5)] {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0));
            let mut panel_rect = egui::Rect::NOTHING;
            let mut heading_rect = egui::Rect::NOTHING;
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                let result = egui::Panel::left("navigation_test")
                    .default_size(width)
                    .resizable(true)
                    .show(ui, |ui| egui::ScrollArea::vertical().show(ui, |ui| nav_heading(ui, level, &text, true)).inner);
                panel_rect = result.response.rect;
                heading_rect = result.inner.rect;
            });

            assert!(panel_rect.width() < width + 20.0, "long heading expanded the {width}px pane to {panel_rect:?}");
            assert!(heading_rect.max.x <= panel_rect.max.x, "heading escaped pane: {heading_rect:?} / {panel_rect:?}");
            assert!(
                output.shapes.iter().any(|s| match &s.shape {
                    egui::Shape::Text(t) => {
                        t.galley.elided && t.galley.rows.last().is_some_and(|r| r.row.glyphs.last().is_some_and(|g| g.chr == '…'))
                    }
                    _ => false,
                }),
                "long heading must render a trailing ellipsis at {width}px"
            );
            output.drop_without_applying_deltas();
        }
    }

    /// Word lists headings flush left, each level indented under its parent (#1199).
    #[test]
    fn headings_align_left_and_indent_by_level() {
        let text_left = |level: u8| {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(900.0, 600.0));
            let mut panel_left = 0.0;
            let output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                let result = egui::Panel::left("navigation_test").default_size(260.0).show(ui, |ui| nav_heading(ui, level, "Kilns", false));
                panel_left = result.response.rect.min.x;
            });
            let x = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == "Kilns" => Some(t.pos.x + t.galley.rect.min.x),
                    _ => None,
                })
                .unwrap();
            output.drop_without_applying_deltas();
            x - panel_left
        };
        let (top, second) = (text_left(0), text_left(1));
        assert!(top < 30.0, "a top-level heading starts at the pane's left edge, not centred: {top}px in");
        assert!((second - top - 14.0).abs() < 0.5, "a level-2 heading is indented 14px under its parent: {top} / {second}");
    }
}

/// Document and user text in the panes draws in bidi order in any interface language (L10N-AR.14):
/// an Arabic heading in an English interface reads right to left.
#[cfg(test)]
mod chrome_text_tests {
    use crate::bidi::visual;
    use crate::{Services, WordApp};
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    fn harness(a: WordApp) -> egui_kittest::Harness<'static, WordApp> {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(5);
        h
    }

    fn app_with(text: &str) -> WordApp {
        let mut a = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Services::default());
        a.run("text.insert", json!({"text": text})).unwrap();
        a
    }

    #[test]
    fn arabic_headings_read_in_order_in_the_navigation_pane() {
        let mut a = app_with("مقدمة 1");
        a.run("para.style", json!({"style": "Heading1"})).unwrap();
        a.run("view.navigationPane", json!({"value": true})).unwrap();
        let h = harness(a);
        let drawn = visual("مقدمة 1", true);
        assert_ne!(drawn, "مقدمة 1", "the probe must need reordering");
        assert!(h.query_by_label(&drawn).is_some(), "the pane draws the heading as {drawn:?}");
        assert!(h.query_by_label("مقدمة 1").is_none(), "not in logical order");
    }

    /// A long Arabic heading is cut at its logical end: the drawn line starts (at the left) with
    /// "…" and keeps the heading's first word, and the pane keeps its width.
    #[test]
    fn long_arabic_headings_are_cut_at_their_logical_end() {
        let text = "مقدمة الفصل الأول عن تاريخ الطباعة ".repeat(12);
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let mut panel = egui::Rect::NOTHING;
        let out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
            panel = egui::Panel::left("navigation_test").default_size(260.0).show(ui, |ui| super::nav_heading(ui, 0, &text, false)).response.rect;
        });
        assert!(panel.width() < 280.0, "{panel:?}");
        let drawn: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None })
            .filter(|t| t.contains('…'))
            .collect();
        out.drop_without_applying_deltas();
        assert_eq!(drawn.len(), 1, "{drawn:?}");
        assert!(drawn[0].starts_with('…') && drawn[0].ends_with("مقدمة"), "cut at the logical end: {drawn:?}");
    }

    /// A hostile heading (a 64 KB paragraph of code spans ending in Arabic) draws in time: only its
    /// first characters are reordered and measured, and the hover text is built only on hover.
    #[test]
    fn huge_headings_draw_in_time() {
        let text = format!("{}ا", "`1` ".repeat(16_000));
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let started = std::time::Instant::now();
        for _ in 0..3 {
            let out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                egui::Panel::left("navigation_test").default_size(260.0).show(ui, |ui| super::nav_heading(ui, 0, &text, false));
            });
            out.drop_without_applying_deltas();
        }
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(3), "three frames took {took:?}");
    }

    #[test]
    fn shown_prefix_cuts_at_a_character_boundary() {
        assert_eq!(super::shown_prefix("مرحبا", 2), "مر");
        assert_eq!(super::shown_prefix("abc", 5), "abc");
        assert_eq!(super::shown_prefix("", 0), "");
        assert_eq!(super::shown_prefix("ab", 0), "");
    }

    /// Editing an Arabic comment shown in bidi order writes the typed text, in reading order,
    /// back to the document: the reordering is display only.
    #[test]
    fn editing_an_arabic_comment_saves_logical_text() {
        let mut a = app_with("Body");
        a.run("select.all", json!({})).unwrap();
        a.run("review.newComment", json!({"text": "راجع هذه الفقرة"})).unwrap();
        a.run("view.commentsPane", json!({"value": true})).unwrap();
        let mut h = harness(a);
        h.get_by_label(&visual("راجع هذه الفقرة", true)).click();
        h.run_steps(4);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.run_steps(1);
        h.get_by_value("راجع هذه الفقرة").type_text("نص جديد (مهم)");
        h.run_steps(2);
        h.key_press(egui::Key::Escape);
        h.run_steps(4);
        let a = h.state();
        let part = a.session.doc.comments.values().next().map(|c| c.part).unwrap();
        assert_eq!(a.session.doc.plain_text(wordcraft_doc::StoryRef::Part(part)).trim_end(), "نص جديد (مهم)");
        assert!(h.query_by_label(&visual("نص جديد (مهم)", true)).is_some(), "shown in bidi order again");
    }

    /// Clicking an Arabic comment's text opens its text field, holding the logical text.
    #[test]
    fn clicking_an_arabic_comment_edits_it() {
        let mut a = app_with("Body");
        a.run("select.all", json!({})).unwrap();
        a.run("review.newComment", json!({"text": "راجع هذه الفقرة"})).unwrap();
        a.run("view.commentsPane", json!({"value": true})).unwrap();
        let mut h = harness(a);
        let drawn = visual("راجع هذه الفقرة", true);
        h.get_by_label(&drawn).click();
        h.run_steps(4);
        assert!(h.query_by_label(&drawn).is_none(), "the text field replaced the drawn text");
        assert!(h.query_by_value("راجع هذه الفقرة").is_some(), "the field holds the comment's text");
    }

    /// Latin headings are drawn exactly as before.
    #[test]
    fn latin_headings_are_unchanged_in_the_navigation_pane() {
        let mut a = app_with("Introduction (part 1)");
        a.run("para.style", json!({"style": "Heading1"})).unwrap();
        a.run("view.navigationPane", json!({"value": true})).unwrap();
        let h = harness(a);
        assert!(h.query_by_label("Introduction (part 1)").is_some());
    }

    /// A find hit in an Arabic paragraph: the result line reads right to left, hit included.
    #[test]
    fn find_results_with_arabic_read_in_order() {
        let text = "هذا تقرير قصير";
        let mut a = app_with(text);
        a.run("view.navigationPane", json!({"value": true})).unwrap();
        a.session.run("edit.find", &json!({"text": "تقرير"})).unwrap();
        a.ui.nav_tab = "results".into();
        let h = harness(a);
        let drawn = visual(text, true);
        assert!(h.query_by_label(&drawn).is_some(), "the result draws as {drawn:?}");
    }

    /// The Styles pane sorts by the names in reading order, not by their draw order: «أمثلة»
    /// comes before «بيان 1», though drawn, «بيان 1» starts with its digit.
    #[test]
    fn styles_pane_sorts_arabic_names_in_reading_order() {
        let mut a = app_with("Body");
        for (id, name) in [("B1", "بيان 1"), ("A", "أمثلة")] {
            a.session.doc.styles.upsert(wordcraft_doc::styles::Style {
                id: id.into(),
                name: name.into(),
                kind: wordcraft_doc::styles::StyleKind::Paragraph,
                ..Default::default()
            });
        }
        a.run("view.stylesPane", json!({"value": true})).unwrap();
        let h = harness(a);
        let y = |label: &str| h.get_by_label(label).rect().min.y;
        assert!(y("أمثلة") < y(&visual("بيان 1", true)), "«أمثلة» is listed first");
    }

    /// The result line takes the paragraph's direction as laid out, including a right-to-left
    /// direction that comes from its style, not only from direct formatting.
    #[test]
    fn find_results_follow_a_style_s_right_to_left_direction() {
        let text = "PDF مرحبا بكم";
        let mut a = app_with(text);
        let para = wordcraft_doc::ParaProps { bidi: Some(true), ..Default::default() };
        a.session.doc.styles.upsert(wordcraft_doc::styles::Style {
            id: "RtlBody".into(),
            name: "RtlBody".into(),
            kind: wordcraft_doc::styles::StyleKind::Paragraph,
            para,
            ..Default::default()
        });
        a.run("para.style", json!({"style": "RtlBody"})).unwrap();
        a.run("view.navigationPane", json!({"value": true})).unwrap();
        a.session.run("edit.find", &json!({"text": "مرحبا"})).unwrap();
        a.ui.nav_tab = "results".into();
        let h = harness(a);
        let drawn = visual(text, true);
        assert_ne!(drawn, visual(text, false), "the probe must tell the two directions apart");
        assert!(h.query_by_label(&drawn).is_some(), "the result draws right to left as {drawn:?}");
    }

    #[test]
    fn arabic_comment_author_and_text_read_in_order() {
        let mut a = app_with("Body");
        a.session.author = "سارة أحمد".into();
        a.run("select.all", json!({})).unwrap();
        a.run("review.newComment", json!({"text": "راجع هذه الفقرة"})).unwrap();
        a.run("view.commentsPane", json!({"value": true})).unwrap();
        let h = harness(a);
        let author = visual("سارة أحمد", true);
        assert!(h.query_by_label(&author).is_some(), "the author draws as {author:?}");
        let text = visual("راجع هذه الفقرة", true);
        assert!(h.query_by_label(&text).is_some(), "the comment text draws as {text:?}");
    }

    /// The Style Inspector's descriptions go through `i18n::labeled`: "Font: Arial" reads label
    /// first under RTL (the label drawn at the right of the value); English is unchanged.
    #[test]
    fn style_descriptions_read_label_first_in_arabic() {
        let ar = crate::i18n::Lang::from_code("ar").unwrap();
        crate::i18n::set_current(ar);
        let (font, size) = (super::describe("font", &json!("Arial")), super::describe("size", &json!(14.0)));
        let (font_label, size_label) = (crate::i18n::tr(ar, "Font").to_string(), crate::i18n::tr(ar, "Size").to_string());
        crate::i18n::set_current(crate::i18n::Lang::EN);
        assert!(font.starts_with("Arial") && font.ends_with(&font_label), "{font:?}");
        assert!(size.ends_with(&size_label), "{size:?}");
        assert_eq!(super::describe("font", &json!("Arial")), "Font: Arial");
        assert_eq!(super::describe("size", &json!(14.0)), "Size: 14 pt");
    }
}
