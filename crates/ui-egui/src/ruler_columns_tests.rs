//! #2196: exercise the painted markers and real pointer drags through the whole window.
use super::*;
use wordcraft_doc::Document;
use wordcraft_engine::Selection;

type H = egui_kittest::Harness<'static, WordApp>;

fn steps(h: &mut H, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

fn window(a: WordApp) -> H {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1440.0, 900.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut WordApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    steps(&mut h, 6);
    h
}

fn focus(h: &mut H, pos: Pos) {
    h.state_mut().session.sel = Selection::caret(pos);
    steps(h, 3);
}

/// Read the three actual painted triangles, ordered by their vertical position on the bar.
fn markers(h: &H) -> [f32; 3] {
    let bottom = h.state().canvas.canvas_rect.unwrap().min.y;
    let mut triangles: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Path(p) if p.points.len() == 3 && p.closed => {
                let bounds = Rect::from_points(&p.points);
                (bounds.min.y >= bottom - RULER && bounds.max.y < bottom).then_some((bounds.min.y, p.points[2].x))
            }
            _ => None,
        })
        .collect();
    triangles.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(triangles.len(), 3, "the first-line, left and right markers: {triangles:?}");
    [triangles[0].1, triangles[1].1, triangles[2].1]
}

fn screen_x(h: &mut H, x: f32) -> f32 {
    page_to_screen(h.state_mut(), 0, x, 72.0).unwrap().x
}

fn check(h: &mut H, x: f32, w: f32, rtl: bool, indents: [f32; 3]) {
    let layout = h.state_mut().session.layout();
    let actual = ruler_text_area(h.state(), &layout, 0).unwrap();
    assert!((actual.0 - x).abs() < 0.01 && (actual.1 - w).abs() < 0.01, "ruler frame {actual:?}, expected ({x}, {w})");
    let [left, first, right] = indents;
    let at = |d| if rtl { x + w - d } else { x + d };
    let expected = [screen_x(h, at(left + first)), screen_x(h, at(left)), screen_x(h, at(w - right))];
    for (actual, expected) in markers(h).into_iter().zip(expected) {
        assert!((actual - expected).abs() < 0.1, "painted marker {actual}, expected {expected}");
    }
}

fn drag(h: &mut H, which: usize, delta: f32) {
    let y = h.state().canvas.canvas_rect.unwrap().min.y - if which == 0 { 15.0 } else { 4.0 };
    let from = pos2(markers(h)[which], y);
    let to = from + vec2(delta, 0.0);
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    h.event(egui::Event::PointerMoved(from));
    steps(h, 2);
    h.event(button(from, true));
    h.step();
    for k in 1..=6 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)));
        h.step();
    }
    h.event(button(to, false));
    steps(h, 3);
}

#[test]
fn column_ruler_paints_and_drags_all_indents_in_both_directions_at_each_zoom() {
    for section_rtl in [false, true] {
        for para_rtl in [false, true] {
            for unequal in [false, true] {
                for zoom in [0.6, 1.5] {
                    // English in every case: language does not choose the section direction.
                    let mut a = WordApp::new(wordcraft_engine::Session::new(Document::from_text("First\n\u{e}Second\nThird")), Default::default());
                    a.run("select.all", json!({})).unwrap();
                    a.run("para.rtl", json!({"value": para_rtl})).unwrap();
                    a.run("para.indents", json!({"left": 9, "firstLine": 9, "right": 9})).unwrap();
                    a.run("layout.sectionRtl", json!({"value": section_rtl})).unwrap();
                    let columns = if unequal { json!({"widths": [[144, 36], [288, 0]]}) } else { json!({"count": 2, "space": 36}) };
                    a.run("layout.columns", columns).unwrap();
                    a.session.view.zoom = zoom;
                    let mut h = window(a);
                    let (first, second) = match (section_rtl, unequal) {
                        (false, false) => ((72.0, 216.0), (324.0, 216.0)),
                        (true, false) => ((324.0, 216.0), (72.0, 216.0)),
                        (false, true) => ((72.0, 144.0), (252.0, 288.0)),
                        (true, true) => ((396.0, 144.0), (72.0, 288.0)),
                    };
                    // Includes movement within the paragraph split by a column break. Looking
                    // up its first placed piece or using the page's first column would fail.
                    for (pos, (x, w)) in [(Pos::body(0, 0), first), (Pos::body(1, 1), second), (Pos::body(1, 0), first), (Pos::body(1, 1), second)] {
                        focus(&mut h, pos);
                        check(&mut h, x, w, para_rtl, [9.0, 9.0, 9.0]);
                    }
                    let scale = h.state().canvas.scale;
                    let direction = if para_rtl { -1.0 } else { 1.0 };
                    drag(&mut h, 1, direction * 18.0 * scale);
                    check(&mut h, second.0, second.1, para_rtl, [27.0, 9.0, 9.0]);
                    drag(&mut h, 0, direction * 18.0 * scale);
                    check(&mut h, second.0, second.1, para_rtl, [27.0, 27.0, 9.0]);
                    drag(&mut h, 2, -direction * 18.0 * scale);
                    check(&mut h, second.0, second.1, para_rtl, [27.0, 27.0, 27.0]);
                    let s = &h.state().session;
                    let props = s.doc.styles.resolve_para(&s.doc.para_at(&s.sel.focus).unwrap().props);
                    assert_eq!((props.indent_left, props.indent_first, props.indent_right), (27.0, 27.0, 27.0));
                    assert_eq!(s.doc.last_section.rtl, section_rtl);
                    focus(&mut h, Pos::body(0, 0));
                    check(&mut h, first.0, first.1, para_rtl, [9.0, 9.0, 9.0]);
                    focus(&mut h, Pos::body(1, 1));
                    check(&mut h, second.0, second.1, para_rtl, [27.0, 27.0, 27.0]);
                }
            }
        }
    }
}

#[test]
fn column_ruler_preserves_single_column_and_cell_frames() {
    let mut a = WordApp::new(wordcraft_engine::Session::new(Document::from_text("Single column")), Default::default());
    let layout = a.session.layout();
    assert_eq!(ruler_text_area(&a, &layout, 0), Some((72.0, 468.0)));
    assert_eq!(ruler_text_area(&a, &layout, usize::MAX), None);
    a.run("layout.columns", json!({"widths": [[144, 36], [288, 0]]})).unwrap();
    a.run("insert.table", json!({"rows": 2, "cols": 2})).unwrap();
    let layout = a.session.layout();
    let cell = crate::table_ruler::text_area(&a, &layout, 0).unwrap();
    assert!(cell.0 > 72.0, "the cell's own margins take precedence over its containing column");
    assert_eq!(ruler_text_area(&a, &layout, 0), Some(cell));
}

#[test]
fn column_ruler_uses_the_caret_section_on_a_continuous_section_page() {
    let mut a = WordApp::new(wordcraft_engine::Session::new(Document::from_text("First\n\u{e}Second")), Default::default());
    a.run("layout.columns", json!({"count": 2, "space": 36})).unwrap();
    a.run("caret.docEnd", json!({})).unwrap();
    a.run("layout.break", json!({"kind": "continuous"})).unwrap();
    a.run("layout.columns", json!({"widths": [[108, 36], [324, 0]]})).unwrap();
    a.run("text.insert", json!({"text": "Third"})).unwrap();
    let mut h = window(a);
    assert_eq!(h.state_mut().session.layout().pages.len(), 1);
    check(&mut h, 72.0, 108.0, false, [0.0; 3]);
    h.state_mut().run("select.text", json!({"text": "Second"})).unwrap();
    steps(&mut h, 3);
    check(&mut h, 324.0, 216.0, false, [0.0; 3]);
}

#[test]
fn column_ruler_keeps_cell_indents_in_a_multicolumn_section() {
    for rtl in [false, true] {
        let mut h = crate::table_resize::tests::table_window();
        h.state_mut().run("layout.columns", json!({"count": 2, "space": 36})).unwrap();
        steps(&mut h, 3);
        let cell = crate::table_resize::tests::cell(&mut h, 0, 1);
        let inside = page_to_screen(h.state_mut(), 0, cell.x + cell.w / 2.0, cell.y + cell.h / 2.0).unwrap();
        h.event(egui::Event::PointerMoved(inside));
        crate::table_resize::tests::button(&h, inside, true);
        h.step();
        crate::table_resize::tests::button(&h, inside, false);
        steps(&mut h, 3);
        h.state_mut().run("para.rtl", json!({"value": rtl})).unwrap();
        steps(&mut h, 3);
        let (x, w) = (cell.x + 5.4, cell.w - 10.8);
        check(&mut h, x, w, rtl, [0.0; 3]);
        let scale = h.state().canvas.scale;
        drag(&mut h, 1, if rtl { -18.0 * scale } else { 18.0 * scale });
        check(&mut h, x, w, rtl, [18.0, 0.0, 0.0]);
    }
}
