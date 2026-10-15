//! Content controls on the page: the frame and title tab of the control the caret is in (every
//! control in Design Mode), check boxes that tick on click, and the list or calendar of a
//! drop-down, combo box or date picker. Everything ends in a `developer.*` command.

use egui::{Align2, Rect, Stroke, Ui, pos2, vec2};
use serde_json::json;
use wordcraft_doc::Pos;
use wordcraft_doc::control::{ControlKind, ControlRange};
use wordcraft_layout::DocLayout;

use crate::WordApp;
use crate::theme::{Tokens, regular};

/// Most controls framed at once (Design Mode in a long form).
const MAX_FRAMED: usize = 400;

/// What the canvas remembers between frames.
#[derive(Default)]
pub struct ControlUi {
    /// Clickable check boxes shown this frame: (screen rect, where the content starts).
    checks: Vec<(Rect, Pos)>,
    /// The list/calendar button of the control the caret is in.
    button: Option<(Rect, Pos)>,
    /// The open list or calendar.
    popup: Option<Popup>,
}

struct Popup {
    /// Content start of the control it belongs to.
    at: Pos,
    /// Where it hangs from (screen).
    anchor: Rect,
    /// Calendar month shown (year, month).
    month: (i64, u32),
}

fn screen(pr: Rect, scale: f32, r: wordcraft_geom::Rect) -> Rect {
    Rect::from_min_size(pos2(pr.min.x + r.x * scale, pr.min.y + r.y * scale), vec2(r.w * scale, r.h * scale))
}

/// The control's content on screen, one rect per page it is on.
fn frames(app: &WordApp, layout: &DocLayout, rects: &[Rect], scale: f32, r: &ControlRange) -> Vec<Rect> {
    let a = r.content_start();
    let mut out: Vec<(usize, Rect)> = Vec::new();
    for (pi, sr) in layout.selection_rects(&app.session.doc, &a, &r.end, app.session.page_hint) {
        let Some(pr) = rects.get(pi) else { continue };
        let s = screen(*pr, scale, sr);
        match out.iter_mut().find(|(p, _)| *p == pi) {
            Some((_, u)) => *u = u.union(s),
            None => out.push((pi, s)),
        }
    }
    if out.is_empty()
        && let Some(c) = layout.caret_on(&a, app.session.page_hint)
        && let Some(pr) = rects.get(c.page)
    {
        out.push((c.page, Rect::from_min_size(pos2(pr.min.x + c.x * scale, pr.min.y + c.top * scale), vec2(4.0, c.height * scale))));
    }
    out.into_iter().map(|(_, r)| r.expand2(vec2(2.0, 1.0))).collect()
}

/// Draw the frames (inside the canvas viewport, after the selection).
pub fn paint(app: &mut WordApp, painter: &egui::Painter, t: &Tokens, layout: &DocLayout, rects: &[Rect], scale: f32) {
    app.controls.checks.clear();
    app.controls.button = None;
    let story = app.session.sel.focus.story;
    let current = app.session.doc.control_at(&app.session.sel.focus);
    let mut shown: Vec<ControlRange> = if app.session.view.design_mode {
        app.session.doc.control_ranges(story).into_iter().take(MAX_FRAMED).collect()
    } else {
        current.clone().into_iter().collect()
    };
    // Check boxes are clickable wherever they are, framed or not.
    let all = if app.session.view.design_mode { shown.clone() } else { app.session.doc.control_ranges(story) };
    for r in all.iter().filter(|r| matches!(r.control.kind, ControlKind::CheckBox { .. })).take(MAX_FRAMED) {
        if let Some(f) = frames(app, layout, rects, scale, r).first() {
            app.controls.checks.push((*f, r.content_start()));
        }
    }
    shown.sort_by_key(|r| r.depth);
    for r in &shown {
        let is_current = current.as_ref().is_some_and(|c| c.start == r.start);
        let fr = frames(app, layout, rects, scale, r);
        let stroke = Stroke::new(1.0, if is_current { t.accent } else { t.accent.gamma_multiply(0.55) });
        for f in &fr {
            painter.rect_stroke(*f, 1.0, stroke, egui::StrokeKind::Outside);
        }
        // The title tab above the first line (its tag in Design Mode when it has no title).
        let title = if r.control.title.is_empty() && app.session.view.design_mode { r.control.tag.as_str() } else { r.control.title.as_str() };
        if let Some(first) = fr.first()
            && !title.is_empty()
        {
            let font = regular(9.5);
            let w = painter.layout_no_wrap(title.to_string(), font.clone(), t.accent_text).size().x + 10.0;
            let tab = Rect::from_min_size(pos2(first.min.x, first.min.y - 14.0), vec2(w.min(320.0), 14.0));
            painter.rect_filled(tab, egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, if is_current { t.accent } else { t.checked });
            let ink = if is_current { t.on_accent } else { t.accent_text };
            painter.with_clip_rect(tab).text(pos2(tab.min.x + 5.0, tab.center().y), Align2::LEFT_CENTER, title, font, ink);
        }
        // The list/calendar button at the right of the current one.
        if is_current
            && matches!(r.control.kind, ControlKind::ComboBox { .. } | ControlKind::DropDown { .. } | ControlKind::Date { .. })
            && let Some(last) = fr.last()
        {
            let b = Rect::from_min_size(pos2(last.max.x + 1.0, last.min.y), vec2(14.0, last.height().max(12.0)));
            painter.rect_filled(b, 1.0, t.checked);
            painter.rect_stroke(b, 1.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
            let c = b.center();
            painter.add(egui::Shape::convex_polygon(
                vec![pos2(c.x - 3.5, c.y - 1.5), pos2(c.x + 3.5, c.y - 1.5), pos2(c.x, c.y + 2.5)],
                t.text,
                Stroke::NONE,
            ));
            app.controls.button = Some((b, r.content_start()));
        }
    }
    if app.controls.popup.as_ref().is_some_and(|p| current.as_ref().is_none_or(|c| c.content_start() != p.at)) {
        app.controls.popup = None;
    }
}

/// After the canvas handled the pointer: a click ticks a check box or opens a list/calendar.
pub fn clicks(app: &mut WordApp, resp: &egui::Response) {
    if !resp.clicked() {
        return;
    }
    let Some(p) = resp.interact_pointer_pos() else { return };
    if let Some((b, at)) = app.controls.button.clone()
        && b.contains(p)
    {
        let today = today();
        let month = date_of(app, &at).map(|(y, m, _)| (y, m)).unwrap_or((today.0, today.1));
        app.controls.popup = if app.controls.popup.is_some() { None } else { Some(Popup { at, anchor: b, month }) };
        return;
    }
    if let Some((_, at)) = app.controls.checks.iter().find(|(r, _)| r.contains(p)).cloned() {
        let at = serde_json::to_value(&at).unwrap_or_default();
        let _ = app.run("developer.toggleCheckBox", json!({"at": at}));
    }
}

fn date_of(app: &WordApp, at: &Pos) -> Option<(i64, u32, u32)> {
    let r = app.session.doc.control_at(at)?;
    match &r.control.kind {
        ControlKind::Date { full_date, .. } => wordcraft_engine::cmd::controls::parse_date(full_date),
        _ => None,
    }
}

fn today() -> (i64, u32, u32) {
    let (y, m, d) = wordcraft_engine::cmd::civil_from_days((wordcraft_engine::cmd::now_unix() / 86_400) as i64);
    (y, m, d)
}

/// The open list or calendar (outside the canvas viewport, above everything).
pub fn popup(app: &mut WordApp, ctx: &egui::Context) {
    let Some(pop) = app.controls.popup.as_ref() else { return };
    let (at, anchor) = (pop.at.clone(), pop.anchor);
    let Some(r) = app.session.doc.control_at(&at) else {
        app.controls.popup = None;
        return;
    };
    let t = Tokens::get(ctx);
    let at_json = serde_json::to_value(&at).unwrap_or_default();
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("content_control_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(anchor.max.x - 180.0, anchor.max.y + 2.0));
    let shown = area.show(ctx, |ui| {
        egui::Frame::popup(ui.style()).fill(t.menu).show(ui, |ui| {
            ui.set_min_width(180.0);
            match &r.control.kind {
                ControlKind::ComboBox { items, .. } | ControlKind::DropDown { items, .. } => {
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        // The document's own entries, each in its own direction. They stay at the
                        // left: right-aligned rows would stretch the popup to the screen's edge.
                        for (i, item) in items.iter().enumerate() {
                            if ui.selectable_label(false, crate::i18n::user_text(&item.display).into_owned()).clicked() {
                                let _ = app.run("developer.chooseItem", json!({"at": at_json, "index": i}));
                                close = true;
                            }
                        }
                    });
                }
                ControlKind::Date { .. } => {
                    if let Some(pop) = app.controls.popup.as_mut()
                        && let Some(day) = calendar(ui, &t, &mut pop.month)
                    {
                        let (y, m) = pop.month;
                        let _ = app.run("developer.setDate", json!({"at": at_json, "date": format!("{y:04}-{m:02}-{day:02}")}));
                        close = true;
                    }
                    if ui.button(tl!("Today")).clicked() {
                        let (y, m, d) = today();
                        let _ = app.run("developer.setDate", json!({"at": at_json, "date": format!("{y:04}-{m:02}-{d:02}")}));
                        close = true;
                    }
                }
                _ => close = true,
            }
        });
    });
    let outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !shown.response.rect.contains(p) && !anchor.contains(p));
    if close || outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.controls.popup = None;
    }
}

/// A month grid (Monday first); returns the day clicked.
fn calendar(ui: &mut Ui, t: &Tokens, month: &mut (i64, u32)) -> Option<u32> {
    let (y, m) = *month;
    let mut picked = None;
    // Back at the start edge, its arrow pointing that way (right under RTL).
    let (back, forward) = if crate::rtl::is_rtl() { ("›", "‹") } else { ("‹", "›") };
    if crate::rtl::is_rtl() {
        // Right-to-left rows take all the width they are given: in a popup (an area as wide as the
        // screen) that is the popup's own width, a week of days.
        ui.set_max_width(ui.min_rect().width().max(7.0 * 22.0 + 6.0 * 2.0));
    }
    crate::rtl::row(ui, |ui| {
        if ui.small_button(back).clicked() {
            *month = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
        }
        ui.label(egui::RichText::new(format!("{y:04}-{m:02}")).strong());
        if ui.small_button(forward).clicked() {
            *month = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        }
    });
    let (y, m) = (y.clamp(1, 9999), m.clamp(1, 12));
    let first = wordcraft_engine::cmd::controls::days_from_civil(y, m, 1);
    // 1970-01-01 was a Thursday: Monday-first column of the 1st.
    let lead = (first + 3).rem_euclid(7) as u32;
    let days = wordcraft_engine::cmd::controls::days_in_month(y, m);
    // A week per row, Monday at the start edge (the right under RTL).
    let rtl = crate::rtl::is_rtl();
    crate::rtl::cell_grid(ui, "cc_calendar", Some(vec2(2.0, 2.0)), 7, (lead + days) as usize, |ui, cell| {
        let cell = cell as u32;
        if cell < lead {
            if rtl {
                // A right-to-left strip has no columns: a blank day takes a day's room.
                ui.allocate_exact_size(vec2(22.0, 18.0), egui::Sense::hover());
            } else {
                ui.label("");
            }
        } else {
            let d = cell - lead + 1;
            let b = egui::Button::new(egui::RichText::new(d.to_string()).color(t.text)).min_size(vec2(22.0, 18.0));
            if ui.add(b).clicked() {
                picked = Some(d);
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    /// The date picker's month in a popup like the real one, in `lang`: (day 1, day 2, popup).
    fn january_2024(lang: &str) -> (egui::Rect, egui::Rect, egui::Rect) {
        let set = move || crate::i18n::set_current(crate::i18n::Lang::from_code(lang).unwrap());
        set();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 600.0)).build_ui(move |ui| {
            set();
            let ctx = ui.ctx().clone();
            let t = super::Tokens::get(&ctx);
            egui::Area::new(egui::Id::new("calendar_popup")).fixed_pos(egui::pos2(300.0, 100.0)).show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(180.0);
                    // 2024-01-01 is a Monday: the 1st and the 2nd share the first row.
                    let _ = super::calendar(ui, &t, &mut (2024, 1));
                });
            });
        });
        h.run_steps(4);
        let day = |d: &str| h.get_by_role_and_label(egui::accesskit::Role::Button, d).rect();
        let (one, two) = (day("1"), day("2"));
        let popup = h.ctx.memory(|m| m.area_rect(egui::Id::new("calendar_popup"))).unwrap();
        crate::i18n::set_current(crate::i18n::Lang::EN);
        (one, two, popup)
    }

    /// Arabic weeks run from the right (Monday at the right), and the popup stays as narrow as
    /// in English.
    #[test]
    fn date_picker_weeks_run_from_the_right_in_arabic() {
        let (one, two, ar) = january_2024("ar");
        assert!(one.left() >= two.right() - 0.5, "arabic: 1 ({one:?}) is right of 2 ({two:?})");
        let (one, two, en) = january_2024("en");
        assert!(one.right() <= two.left() + 0.5, "english: 1 ({one:?}) is left of 2 ({two:?})");
        assert!(ar.is_finite() && ar.width() <= en.width() + 40.0, "arabic popup {ar:?}, english {en:?}");
    }
}
