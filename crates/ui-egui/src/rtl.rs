//! Direction-aware replacements for the LTR-only egui idioms (`ui.horizontal`,
//! `Layout::right_to_left`, `Align2::LEFT_*`, `Grid` rows, absolute painter x).
//! The direction is `i18n::current().is_rtl()`, which is per thread and set every frame.

// Used by the mirroring tasks (7-13).
#![cfg_attr(not(test), allow(dead_code))]

use egui::{
    Align, Align2, Button, InnerResponse, IntoAtoms, Layout, Popup, Rect, RectAlign, Response, RichText, Ui, Vec2, Widget as _, WidgetText, vec2,
};

/// Is the interface drawn right to left?
pub fn is_rtl() -> bool {
    crate::i18n::current().is_rtl()
}

/// `ui.horizontal` that starts at the visual start edge (right under RTL). Allocates exactly
/// as `Ui::horizontal` does.
///
/// A sizing pass (an auto-sized window's first frame, a grid's first frame) only measures, so it
/// lays the row out left to right: right to left, content wider than the room it was given would
/// spill past the left edge and the window would keep that extra width.
pub fn row<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let size = vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
    let layout = if ui.is_sizing_pass() { Layout::left_to_right(Align::Center) } else { row_layout(Align::Center) };
    ui.allocate_ui_with_layout(size, layout, add)
}

/// `ui.vertical` whose items line up on the start edge (the right under RTL). Inside a
/// right-to-left row a plain `ui.vertical` would land at the far left of the free space.
/// A sizing pass lays it out as `ui.vertical` (see [`row`]): right-aligned content measures as
/// wide as the room it was given.
pub fn column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    if is_rtl() && !ui.is_sizing_pass() {
        ui.scope_builder(egui::UiBuilder::new().layout(Layout::top_down(Align::Max)), add)
    } else {
        ui.vertical(add)
    }
}

/// Content that always lays out from the left of the free space (an egui `Grid` or `ScrollArea`),
/// placed at the cursor of a right-to-left row: it gets a box exactly as wide as it measured last
/// frame (a changed width discards the frame and lays it out again). English: `add(ui)` as is.
///
/// The content runs in a child that does not size the box: the box is always the stored width, so
/// a pass where the content came out wider than last time doesn't push past the box's right edge
/// and widen an auto-sized window for good (the next pass gets the new width).
pub fn fitted<R>(ui: &mut Ui, id_salt: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    if !is_rtl() {
        return add(ui);
    }
    let id = ui.id().with(("rtl_fitted", id_salt));
    let known = ui.data(|d| d.get_temp::<f32>(id));
    let width = known.unwrap_or_else(|| ui.available_width());
    let r = ui.allocate_ui_with_layout(vec2(width, ui.available_height()), Layout::top_down(Align::Min), |ui| {
        let mut content = ui.new_child(egui::UiBuilder::new().max_rect(ui.max_rect()).layout(Layout::top_down(Align::Min)));
        let inner = add(&mut content);
        let used = content.min_rect();
        ui.set_min_size(vec2(known.unwrap_or(used.width()), used.height()));
        (inner, used.width())
    });
    let (inner, got) = r.inner;
    if known.is_none_or(|k| (k - got).abs() > 0.5) {
        ui.data_mut(|d| d.insert_temp(id, got));
        ui.ctx().request_discard("rtl: fitted width changed");
    }
    inner
}

/// The start-to-end row layout: left to right, or right to left under RTL.
/// Note: with_layout with Align::Center takes the full available height unless the caller caps it.
pub fn row_layout(align: Align) -> Layout {
    if is_rtl() { Layout::right_to_left(align) } else { Layout::left_to_right(align) }
}

/// The end-to-start row layout (pushes content to the end edge): the opposite of [`row_layout`].
pub fn end_layout(align: Align) -> Layout {
    if is_rtl() { Layout::left_to_right(align) } else { Layout::right_to_left(align) }
}

/// Painter text anchor at the start edge (`LEFT_*`, or `RIGHT_*` under RTL).
pub fn start_align2(v: Align) -> Align2 {
    Align2([if is_rtl() { Align::RIGHT } else { Align::LEFT }, v])
}

/// Painter text anchor at the end edge (`RIGHT_*`, or `LEFT_*` under RTL).
pub fn end_align2(v: Align) -> Align2 {
    Align2([if is_rtl() { Align::LEFT } else { Align::RIGHT }, v])
}

/// Mirror an absolute x inside `outer` under RTL.
pub fn x(outer: Rect, x: f32) -> f32 {
    if is_rtl() { outer.right() - (x - outer.left()) } else { x }
}

/// Mirror a rect inside `outer` under RTL.
pub fn rect(outer: Rect, r: Rect) -> Rect {
    if is_rtl() { Rect::from_min_max(egui::pos2(x(outer, r.max.x), r.min.y), egui::pos2(x(outer, r.min.x), r.max.y)) } else { r }
}

/// `egui::Grid::new(id_salt).num_columns(num_columns)` for rows built by [`grid_row`]. Under RTL
/// the column count is left unset: egui gives the declared last column every point up to the
/// grid's right edge, and a right-to-left cell there pins its content to that edge, so the column
/// never fits its content (captions overlap their fields, or sit far from them).
pub fn grid(id_salt: impl egui::AsIdSalt, num_columns: usize) -> egui::Grid {
    let g = egui::Grid::new(id_salt);
    if is_rtl() { g } else { g.num_columns(num_columns) }
}

/// One `Grid` row: cells in reading order (reversed under RTL), each aligned to its column's
/// start edge, then `end_row`.
pub fn grid_row<const N: usize>(ui: &mut Ui, cells: [&mut dyn FnMut(&mut Ui); N]) {
    if is_rtl() {
        for cell in cells.into_iter().rev() {
            // Height-capped like `row`, or the cell would take the grid's whole remaining height.
            let size = vec2(ui.available_width(), ui.spacing().interact_size.y);
            // A sizing pass (a window's first frame, a grid's first frame) only measures, and its
            // cells are narrower than their content: laid out right to left the content would
            // spill left of the grid, and an auto-sized window would keep that extra width.
            let dir = if ui.is_sizing_pass() { Layout::left_to_right(Align::Center) } else { Layout::right_to_left(Align::Center) };
            ui.allocate_ui_with_layout(size, dir, |ui| cell(ui));
        }
    } else {
        // English: exactly the plain `label; field; end_row()` it replaces.
        for cell in cells {
            cell(ui);
        }
    }
    ui.end_row();
}

/// A grid of `n` uniform cells, `per_row` to a row, built by `cell(ui, index)`, with `spacing`
/// (egui's item spacing when `None`). English: exactly the `egui::Grid` (an `end_row` after every
/// `per_row` cells). RTL: each row is a right-to-left strip, so the first cell is the right one.
/// Use it for galleries and colour grids inside menus: egui's `Grid` always lays out from the left,
/// and inside a right-aligned menu ([`menu_layout`]) it sized the menu to infinity (NaN rects).
pub fn cell_grid(ui: &mut Ui, id_salt: impl egui::AsIdSalt, spacing: Option<Vec2>, per_row: usize, n: usize, mut cell: impl FnMut(&mut Ui, usize)) {
    let per_row = per_row.max(1);
    if !is_rtl() {
        let mut grid = egui::Grid::new(id_salt);
        if let Some(s) = spacing {
            grid = grid.spacing(s);
        }
        grid.show(ui, |ui| {
            for i in 0..n {
                cell(ui, i);
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
        return;
    }
    let s = spacing.unwrap_or(ui.spacing().item_spacing);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = s.y;
        let mut start = 0;
        while start < n {
            let end = start.saturating_add(per_row).min(n);
            // Top-aligned (`row` centres on one control height; cells can be taller).
            ui.allocate_ui_with_layout(vec2(ui.available_size_before_wrap().x, 0.0), Layout::right_to_left(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.x = s.x;
                for i in start..end {
                    cell(ui, i);
                }
            });
            start = end;
        }
    });
}

/// Layout for menu items: text aligned to the start edge.
pub fn menu_layout() -> Layout {
    Layout::top_down_justified(if is_rtl() { Align::Max } else { Align::Min })
}

// ---- Menus, tooltips and check boxes (L10N-AR.9) ----
//
// Every helper below is the stock egui call in English (same code path, so the same pixels). Under
// RTL a menu hangs from its button's right edge and its items line up on the right, a sub-menu opens
// to the left with its arrow (⏴) at the item's left, a shortcut sits at the item's left end, a
// tooltip hangs from the widget's right edge with right-aligned rows, and a check box draws its box
// right of its label.

/// [`egui::Popup::menu`] for a button. RTL: the menu's right edge sits under the button's right
/// edge (`RectAlign::BOTTOM_END`; egui still flips it when it does not fit) and items line up on the
/// right ([`menu_layout`]).
pub fn menu(resp: &Response) -> Popup<'static> {
    let p = Popup::menu(resp);
    if is_rtl() { p.layout(menu_layout()).align(RectAlign::BOTTOM_END) } else { p }
}

/// [`egui::Popup::context_menu`] (opens at the pointer on a secondary click). RTL: the menu opens
/// to the left of the pointer, items on the right.
pub fn context_menu(resp: &Response) -> Popup<'static> {
    let p = Popup::context_menu(resp);
    if is_rtl() { p.layout(menu_layout()).align(RectAlign::BOTTOM_END) } else { p }
}

/// `resp.context_menu(add)` through [`context_menu`].
pub fn on_context_menu(resp: &Response, add: impl FnOnce(&mut Ui)) -> Option<InnerResponse<()>> {
    context_menu(resp).show(add)
}

/// A menu item with a shortcut (`Button::new(label).shortcut_text(sc)`). RTL: the label at the
/// right, the shortcut (weak) at the left end.
pub fn shortcut_item<'a>(label: impl Into<WidgetText>, shortcut: impl Into<String>) -> Button<'a> {
    let (label, shortcut) = (label.into(), shortcut.into());
    if is_rtl() { Button::new(label).left_text(RichText::new(shortcut).weak()) } else { Button::new(label).shortcut_text(shortcut) }
}

/// A menu item led by a mark (a check mark): `Button::new((mark, label))`. RTL: the mark at the
/// right, before the label in reading order.
pub fn marked_item<'a>(mark: RichText, label: impl Into<WidgetText>) -> Button<'a> {
    let label = label.into();
    if is_rtl() { Button::new((label, mark)) } else { Button::new((mark, label)) }
}

/// `ui.menu_button`: a button opening a menu, or inside a menu a sub-menu item. RTL: the menu
/// through [`menu`]; a sub-menu opens to the left of its item and its arrow (⏴) is at the left.
pub fn sub_menu<'a, R>(ui: &mut Ui, atoms: impl IntoAtoms<'a>, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<Option<R>> {
    if !is_rtl() {
        return ui.menu_button(atoms, add);
    }
    if !egui::containers::menu::is_in_menu(ui) {
        let resp = Button::new(atoms).ui(ui);
        let inner = menu(&resp).show(add);
        return InnerResponse::new(inner.map(|i| i.inner), resp);
    }
    rtl_sub_menu(ui, Button::new(atoms).left_text("⏴"), add)
}

/// egui's `SubMenuButton` + `SubMenu::show` with the sub-menu placed `LEFT_START` and laid out by
/// [`menu_layout`]. egui hard-codes `RIGHT_START` there.
///
/// Ported from egui 0.36.2 (MIT OR Apache-2.0), Copyright (c) 2018-2021 Emil Ernerfeldt.
fn rtl_sub_menu<R>(ui: &mut Ui, button: Button<'_>, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<Option<R>> {
    use egui::containers::menu::{MenuConfig, MenuState, SubMenu};
    let my_id = ui.next_auto_id();
    let open = MenuState::from_ui(ui, |state, _| state.open_item == Some(SubMenu::id_from_widget_id(my_id)));
    let inactive = ui.style().visuals.widgets.inactive;
    if open {
        ui.style_mut().visuals.widgets.inactive = ui.style().visuals.widgets.open;
    }
    let button_response = button.ui(ui);
    ui.style_mut().visuals.widgets.inactive = inactive;

    let frame = egui::Frame::menu(ui.style());
    let id = SubMenu::id_from_widget_id(button_response.id);
    let (open_item, menu_id) = MenuState::from_ui(ui, |state, stack| (state.open_item, stack.id));
    let menu_config = MenuConfig::find(ui);
    // `MenuConfig::bar` is private; `find` returns the enclosing menu's config, which is not a bar.
    let Some(menu_root_response) = ui.ctx().read_response(menu_id) else {
        return InnerResponse::new(None, button_response);
    };
    let hover_pos = ui.ctx().pointer_hover_pos();
    let menu_rect = menu_root_response.rect - frame.total_margin();
    let is_hovering_menu = hover_pos.is_some_and(|pos| ui.ctx().layer_id_at(pos) == Some(menu_root_response.layer_id) && menu_rect.contains(pos));
    let is_any_open = open_item.is_some();
    let mut is_open = open_item == Some(id);
    let was_open = is_open;
    let mut set_open = None;
    let button_rect = button_response.rect.expand2(ui.style().spacing.item_spacing / 2.0);
    let is_hovered = hover_pos.is_some_and(|pos| button_rect.contains(pos));
    let clicked = button_response.clicked();
    let clicked_by_pointer = button_response.clicked_by(egui::PointerButton::Primary);
    if ui.is_enabled() && is_open && clicked && !clicked_by_pointer {
        set_open = Some(false);
        is_open = false;
    }
    if ui.is_enabled() && ((!was_open && clicked) || (is_hovered && !is_any_open)) {
        set_open = Some(true);
        is_open = true;
        MenuState::from_id(ui.ctx(), menu_id, |state| state.open_item = None);
    }
    let gap = frame.total_margin().sum().x / 2.0 + 2.0;
    let mut response = button_response.clone();
    response.interact_rect = response.interact_rect.expand2(vec2(0.0, frame.total_margin().sum().y / 2.0));
    let close_behavior = menu_config.close_behavior;
    let popup_response = Popup::from_response(&response)
        .id(id)
        .open(is_open)
        .align(RectAlign::LEFT_START)
        .layout(menu_layout())
        .gap(gap)
        .style(menu_config.style.clone())
        .frame(frame)
        .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
        .info(egui::UiStackInfo::new(egui::UiKind::Menu).with_tag_value(MenuConfig::MENU_CONFIG_TAG, menu_config))
        .show(|ui| {
            if button_response.clicked() || button_response.is_pointer_button_down_on() {
                ui.ctx().move_to_top(ui.layer_id());
            }
            add(ui)
        });
    if let Some(popup_response) = &popup_response {
        let is_deepest = MenuState::is_deepest_open_sub_menu(ui.ctx(), id);
        let clicked_outside = is_deepest && popup_response.response.clicked_elsewhere() && menu_root_response.clicked_elsewhere();
        let clicked_inside = is_deepest
            && !button_response.clicked()
            && response.ctx.input(|i| i.pointer.any_click())
            && hover_pos.is_some_and(|pos| popup_response.response.interact_rect.contains(pos));
        let click_close = match close_behavior {
            egui::PopupCloseBehavior::CloseOnClick => clicked_outside || clicked_inside,
            egui::PopupCloseBehavior::CloseOnClickOutside => clicked_outside,
            egui::PopupCloseBehavior::IgnoreClicks => false,
        };
        if click_close {
            set_open = Some(false);
            ui.close();
        }
        let towards = ui.input(|i| i.pointer.is_moving_towards_rect(&popup_response.response.rect));
        if towards {
            ui.request_repaint();
        }
        if is_open && !is_hovered && !popup_response.response.contains_pointer() && !towards && is_hovering_menu {
            set_open = Some(false);
        }
        if popup_response.response.should_close() {
            ui.close();
        }
        if ui.will_parent_close() {
            ui.data_mut(|data| data.remove_by_type::<MenuState>());
        }
    }
    if let Some(set_open) = set_open {
        MenuState::from_id(ui.ctx(), menu_id, |state| state.open_item = set_open.then_some(id));
    }
    InnerResponse::new(popup_response.map(|r| r.inner), button_response)
}

/// [`egui::Tooltip::for_enabled`]. RTL: the tooltip hangs from the widget's right edge and its rows
/// line up on the right.
pub fn tooltip(resp: &Response) -> egui::Tooltip<'static> {
    let mut t = egui::Tooltip::for_enabled(resp);
    if is_rtl() {
        t = t.layout(Layout::top_down(Align::Max));
        t.popup = t.popup.align(RectAlign::BOTTOM_END);
    }
    t
}

/// `resp.on_hover_ui(add)` through [`tooltip`].
pub fn on_hover_ui(resp: Response, add: impl FnOnce(&mut Ui)) -> Response {
    tooltip(&resp).show(add);
    resp
}

/// `resp.on_hover_text(text)`. RTL: the text wraps with [`wrapped_label`], so a long tip reads from
/// its top line down, every row ending at the right.
pub fn hover_text(resp: Response, text: impl Into<String>) -> Response {
    let text = text.into();
    if !is_rtl() {
        return resp.on_hover_text(text);
    }
    on_hover_ui(resp, |ui| {
        // As `on_hover_text`: keep Area auto-sizing from shrinking the tooltip.
        ui.set_max_width(ui.spacing().tooltip_width);
        wrapped_label(ui, &text, |s| RichText::new(s));
    })
}

/// [`hover_text`] as a method, for `resp.on_hover_text(…)` chains: `resp.rtl_hover_text(…)`.
pub trait HoverText {
    fn rtl_hover_text(self, text: impl Into<String>) -> Response;
}

impl HoverText for Response {
    fn rtl_hover_text(self, text: impl Into<String>) -> Response {
        hover_text(self, text)
    }
}

/// A label that wraps at the available width. English: `ui.label(style(text))`.
///
/// RTL: `text` is in draw order, as the catalog and `fmt` give it. Text that fits on one line is
/// drawn as it is. Longer text is broken into rows by `bidi::wrap` from its logical order and
/// drawn one row under the other, right-aligned, so the first line of the sentence is on top
/// (egui's own wrapping would cut the draw-order string and put the end of the sentence first).
///
/// The logical text of a catalog translation comes from the catalog itself ([`i18n::logical`]).
/// Draw order can't be undone in general, so composed text (a `fmt` result, a translation with a
/// shortcut appended) falls back to `bidi::visual` run on the draw-order text: right for plain
/// sentences, approximate when numbers, isolated values or left-to-right runs with trailing
/// neutrals sit inside (`اختر 2 Columns` does not come back).
///
/// [`i18n::logical`]: crate::i18n::logical
pub fn wrapped_label(ui: &mut Ui, text: &str, style: impl Fn(&str) -> RichText) -> Response {
    if !is_rtl() {
        return ui.label(style(text));
    }
    let width = ui.available_width();
    let measure =
        |s: &str| WidgetText::from(style(s)).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::FontSelection::Default).size().x;
    let rows = if measure(text) <= width {
        vec![text.to_string()]
    } else {
        let logical = crate::i18n::logical(text).map_or_else(|| crate::bidi::visual(text, true), std::borrow::Cow::Borrowed);
        crate::bidi::wrap(&logical, true, width, measure)
    };
    column(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for row in rows {
            ui.add(egui::Label::new(style(&row)).wrap_mode(egui::TextWrapMode::Extend));
        }
    })
    .response
}

/// A check box that reads in the interface direction. English: exactly `ui.checkbox`. RTL: the box
/// is right of its label (drawn as egui draws its own, mirrored). Dialogs use it too.
///
/// The RTL drawing is ported from egui 0.36.2's `Checkbox` (MIT OR Apache-2.0), Copyright (c)
/// 2018-2021 Emil Ernerfeldt.
pub fn checkbox<'a>(ui: &mut Ui, checked: &'a mut bool, atoms: impl IntoAtoms<'a>) -> Response {
    if !is_rtl() {
        return ui.checkbox(checked, atoms);
    }
    rtl_checkbox(ui, checked, false, atoms)
}

/// A check box shown as neither on nor off (a setting that differs across the selection); a
/// click reports `clicked` and the caller decides. English: exactly
/// `ui.add(egui::Checkbox::new(&mut false, atoms).indeterminate(true))`. RTL: the box right of
/// its label, with egui's dash in it.
pub fn checkbox_indeterminate(ui: &mut Ui, text: &str) -> Response {
    let mut off = false;
    if !is_rtl() {
        return ui.add(egui::Checkbox::new(&mut off, text).indeterminate(true));
    }
    rtl_checkbox(ui, &mut off, true, text)
}

/// The right-to-left check box of [`checkbox`] and [`checkbox_indeterminate`].
fn rtl_checkbox<'a>(ui: &mut Ui, checked: &mut bool, indeterminate: bool, atoms: impl IntoAtoms<'a>) -> Response {
    let id = ui.next_auto_id();
    let state = ui.ctx().read_response(id).map(|r| r.widget_state()).unwrap_or_default();
    let egui::widget_style::CheckboxStyle { check_size, checkbox_frame, checkbox_size, frame, check_stroke, text_style } =
        ui.style().checkbox_style(&egui::widget_style::Classes::default(), state);
    let mut min_size = Vec2::splat(ui.spacing().interact_size.y);
    min_size.y = min_size.y.max(checkbox_size);
    let icon_size = vec2(checkbox_size, checkbox_size.max(min_size.y));
    let rect_id = egui::Id::new("wc::rtl_checkbox");
    let mut atoms = atoms.into_atoms();
    let text = atoms.text().map(String::from);
    atoms.push_right(egui::Atom::custom(rect_id, icon_size));
    let mut prepared = egui::AtomLayout::new(atoms).sense(egui::Sense::click()).min_size(min_size).frame(frame).allocate(ui);
    if prepared.response.clicked() {
        *checked = !*checked;
        prepared.response.mark_changed();
    }
    let on = *checked;
    prepared.response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, text.as_deref().unwrap_or("")));
    if !ui.is_rect_visible(prepared.response.rect) {
        return prepared.response;
    }
    prepared.fallback_text_color = text_style.color;
    let shown = prepared.paint(ui);
    if let Some(rect) = shown.rect(rect_id) {
        let big = Rect::from_center_size(egui::pos2(rect.right() - checkbox_size / 2.0, rect.center().y), Vec2::splat(checkbox_size));
        let small = Rect::from_center_size(big.center(), Vec2::splat(check_size));
        ui.painter().add(egui::epaint::RectShape::new(
            big.expand(checkbox_frame.inner_margin.left.into()),
            checkbox_frame.corner_radius,
            checkbox_frame.fill,
            checkbox_frame.stroke,
            egui::StrokeKind::Inside,
        ));
        if indeterminate {
            // egui's dash across the box.
            ui.painter().add(egui::Shape::hline(small.x_range(), small.center().y, check_stroke));
        } else if on {
            // The check mark is a tick, not a direction: drawn as in English.
            ui.painter().add(egui::Shape::line(
                vec![
                    egui::pos2(small.left(), small.center().y),
                    egui::pos2(small.center().x, small.bottom()),
                    egui::pos2(small.right(), small.top()),
                ],
                check_stroke,
            ));
        }
    }
    shown.response
}

/// [`checkbox`], greyed out unless `enabled`. English: exactly
/// `ui.add_enabled(enabled, egui::Checkbox::new(checked, atoms))`.
pub fn checkbox_enabled<'a>(ui: &mut Ui, enabled: bool, checked: &'a mut bool, atoms: impl IntoAtoms<'a>) -> Response {
    if is_rtl() {
        ui.add_enabled_ui(enabled, |ui| checkbox(ui, checked, atoms)).inner
    } else {
        ui.add_enabled(enabled, egui::Checkbox::new(checked, atoms))
    }
}

/// A radio button that reads in the interface direction. English: exactly `ui.radio`. RTL: the
/// circle is right of its label (drawn as egui draws its own, mirrored). For dialogs.
///
/// The RTL drawing is ported from egui 0.36.2's `RadioButton` (MIT OR Apache-2.0), Copyright (c)
/// 2018-2021 Emil Ernerfeldt.
pub fn radio<'a>(ui: &mut Ui, checked: bool, atoms: impl IntoAtoms<'a>) -> Response {
    if !is_rtl() {
        return ui.radio(checked, atoms);
    }
    let icon_width = ui.spacing().icon_width;
    let mut min_size = Vec2::splat(ui.spacing().interact_size.y);
    min_size.y = min_size.y.max(icon_width);
    let icon_size = vec2(icon_width, icon_width.max(min_size.y));
    let rect_id = egui::Id::new("wc::rtl_radio");
    let mut atoms = atoms.into_atoms();
    let text = atoms.text().map(String::from);
    atoms.push_right(egui::Atom::custom(rect_id, icon_size));
    let mut prepared = egui::AtomLayout::new(atoms).sense(egui::Sense::click()).min_size(min_size).allocate(ui);
    prepared
        .response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, ui.is_enabled(), checked, text.as_deref().unwrap_or("")));
    if !ui.is_rect_visible(prepared.response.rect) {
        return prepared.response;
    }
    let visuals = *ui.style().interact(&prepared.response);
    prepared.fallback_text_color = visuals.text_color();
    let shown = prepared.paint(ui);
    if let Some(rect) = shown.rect(rect_id) {
        // egui's icon rectangles hug the rect's left edge: move them to its right edge.
        let (small, big) = ui.spacing().icon_rectangles(rect);
        let shift = vec2(rect.right() - big.right(), 0.0);
        let (small, big) = (small.translate(shift), big.translate(shift));
        let painter = ui.painter();
        painter.add(egui::epaint::CircleShape {
            center: big.center(),
            radius: big.width() / 2.0 + visuals.expansion,
            fill: visuals.bg_fill,
            stroke: visuals.bg_stroke,
        });
        if checked {
            painter.add(egui::epaint::CircleShape {
                center: small.center(),
                radius: small.width() / 3.0,
                fill: visuals.fg_stroke.color,
                stroke: Default::default(),
            });
        }
    }
    shown.response
}

/// `ui.radio_value` through [`radio`]: selects `alternative` when clicked. English: exactly
/// `ui.radio_value`.
pub fn radio_value<'a, V: PartialEq>(ui: &mut Ui, current: &mut V, alternative: V, atoms: impl IntoAtoms<'a>) -> Response {
    if !is_rtl() {
        return ui.radio_value(current, alternative, atoms);
    }
    let mut r = radio(ui, *current == alternative, atoms);
    if r.clicked() && *current != alternative {
        *current = alternative;
        r.mark_changed();
    }
    r
}

/// `ui.horizontal_top`: a row of blocks of several lines (columns of controls, lists) whose
/// tops line up, from the start edge (the right under RTL). English: exactly
/// `ui.horizontal_top`. A sizing pass lays it out left to right (see [`row`]).
///
/// [`grid_row`] and [`row`] centre their cells on one control height, so a taller cell would
/// stick out above its row; this one is top-aligned and as tall as its tallest block.
pub fn row_top<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    if !is_rtl() {
        return ui.horizontal_top(add);
    }
    let size = vec2(ui.available_size_before_wrap().x.max(0.0), 0.0);
    // A sizing pass lays it out left to right like `row` and `grid_row`, top-aligned: inside a
    // right-aligned parent `horizontal_top` would already run right to left.
    let layout =
        if ui.is_sizing_pass() { Layout::left_to_right(Align::Center).with_cross_align(Align::Min) } else { Layout::right_to_left(Align::Min) };
    ui.allocate_ui_with_layout(size, layout, add)
}

/// [`fitted`] content (a `Grid`) at the start edge of a Ui that lays out from the left, such as a
/// `ScrollArea`'s content: at the right under RTL. English: `add(ui)` as is.
pub fn start_fitted<R>(ui: &mut Ui, id_salt: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    if !is_rtl() {
        return add(ui);
    }
    ui.with_layout(Layout::top_down(Align::Max), |ui| fitted(ui, id_salt, add)).inner
}

/// Test helper: the screen rect of every text shape drawn last frame whose text is `text`.
#[cfg(test)]
pub(crate) fn text_rects(out: &egui::FullOutput, text: &str) -> Vec<Rect> {
    fn walk(shape: &egui::Shape, text: &str, found: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == text => found.push(t.galley.rect.translate(t.pos.to_vec2())),
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, text, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for s in &out.shapes {
        walk(&s.shape, text, &mut found);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Lang};
    use egui_kittest::kittest::Queryable;

    /// Run `f` with the interface language set, restoring English afterwards.
    fn with_lang<T>(code: &str, f: impl FnOnce() -> T) -> T {
        i18n::set_current(Lang::from_code(code).unwrap_or(Lang::EN));
        let out = f();
        i18n::set_current(Lang::EN);
        out
    }

    fn outer() -> Rect {
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 10.0))
    }

    #[test]
    fn x_and_rect_mirror_inside_the_outer_rect() {
        let r = Rect::from_min_max(egui::pos2(10.0, 0.0), egui::pos2(30.0, 10.0));
        with_lang("ar", || {
            assert!(is_rtl());
            assert_eq!(x(outer(), 10.0), 90.0);
            assert_eq!(rect(outer(), r), Rect::from_min_max(egui::pos2(70.0, 0.0), egui::pos2(90.0, 10.0)));
        });
        assert!(!is_rtl());
        assert_eq!(x(outer(), 10.0), 10.0);
        assert_eq!(rect(outer(), r), r);
    }

    fn two_labels(code: &str) -> (f32, f32) {
        with_lang(code, || {
            let mut h = egui_kittest::Harness::new_ui(|ui| {
                row(ui, |ui| {
                    ui.label("A");
                    ui.label("B");
                });
            });
            h.run();
            (h.get_by_label("A").rect().center().x, h.get_by_label("B").rect().center().x)
        })
    }

    #[test]
    fn row_places_the_first_widget_at_the_right_in_rtl() {
        let (a, b) = two_labels("ar");
        assert!(a > b, "arabic: A ({a}) is right of B ({b})");
        let (a, b) = two_labels("en");
        assert!(a < b, "english: A ({a}) is left of B ({b})");
    }

    fn grid_cells(code: &str) -> (f32, f32) {
        with_lang(code, || {
            let mut h = egui_kittest::Harness::new_ui(|ui| {
                egui::Grid::new("g").show(ui, |ui| {
                    grid_row(
                        ui,
                        [
                            &mut |ui| {
                                ui.label("L");
                            },
                            &mut |ui| {
                                ui.label("F");
                            },
                        ],
                    );
                });
            });
            h.run();
            (h.get_by_label("L").rect().center().x, h.get_by_label("F").rect().center().x)
        })
    }

    #[test]
    fn grid_row_reverses_cells_in_rtl() {
        let (l, f) = grid_cells("ar");
        assert!(l > f, "arabic: L ({l}) is right of F ({f})");
        let (l, f) = grid_cells("en");
        assert!(l < f, "english: L ({l}) is left of F ({f})");
    }

    #[test]
    fn layouts_and_alignments_follow_direction() {
        assert!(!row_layout(Align::Center).prefer_right_to_left());
        assert!(end_layout(Align::Center).prefer_right_to_left());
        assert_eq!(start_align2(Align::Min), Align2::LEFT_TOP);
        assert_eq!(end_align2(Align::Center), Align2::RIGHT_CENTER);
        assert_eq!(menu_layout().cross_align, Align::Min);
        with_lang("ar", || {
            assert!(row_layout(Align::Center).prefer_right_to_left());
            assert!(!end_layout(Align::Center).prefer_right_to_left());
            assert_eq!(start_align2(Align::Min), Align2::RIGHT_TOP);
            assert_eq!(end_align2(Align::Center), Align2::LEFT_CENTER);
            assert_eq!(menu_layout().cross_align, Align::Max);
        });
    }

    /// (label y, grid height) for a grid row built by `grid_row` or by plain cells.
    fn grid_metrics(code: &str, helper: bool) -> (f32, f32) {
        use std::{cell::Cell, rc::Rc};
        let height = Rc::new(Cell::new(0.0_f32));
        let h2 = height.clone();
        let y = with_lang(code, || {
            let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(300.0, 400.0)).build_ui(move |ui| {
                let r = egui::Grid::new("g").show(ui, |ui| {
                    if helper {
                        grid_row(
                            ui,
                            [
                                &mut |ui| {
                                    ui.label("L");
                                },
                                &mut |ui| {
                                    ui.label("F");
                                },
                            ],
                        );
                    } else {
                        ui.label("L");
                        ui.label("F");
                        ui.end_row();
                    }
                });
                h2.set(r.response.rect.height());
            });
            h.run();
            h.get_by_label("L").rect().center().y
        });
        (y, height.get())
    }

    // ---- Menus, tooltips and check boxes (L10N-AR.9) ----

    const LONG_TIP: &str = "يعرض هذا الزر قائمة الأوامر التي تنسق النص المحدد في المستند الحالي بسرعة وسهولة ويمكنك تخصيصها من خيارات الشريط في أي وقت تريده دون أن تفقد أي شيء من عملك أو من تنسيقه";

    #[test]
    fn rtl_tooltips_wrap_with_the_first_line_on_top() {
        with_lang("ar", || {
            let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(900.0, 500.0)).build_ui(|ui| {
                let r = ui.button("B");
                hover_text(r, crate::bidi::visual(LONG_TIP, true));
            });
            h.ctx.all_styles_mut(|s| {
                s.interaction.tooltip_delay = 0.0;
                s.interaction.show_tooltips_only_when_still = false;
            });
            h.run();
            h.get_by_label("B").hover();
            h.run_steps(4);
            let style = h.ctx.global_style();
            let font = egui::TextStyle::Body.resolve(&style);
            let measure = |s: &str| h.ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), egui::Color32::WHITE).size().x);
            // The catalog stores the tip in draw order; the rows are the logical text wrapped.
            let rows = crate::bidi::wrap(LONG_TIP, true, style.spacing.tooltip_width, measure);
            assert_eq!(crate::bidi::visual(&crate::bidi::visual(LONG_TIP, true), true), LONG_TIP, "draw order converts back");
            assert!(rows.len() >= 2, "the tip is long enough to wrap: {rows:?}");
            let rects: Vec<Rect> = rows
                .iter()
                .map(|row| {
                    let found = text_rects(h.output(), row);
                    assert_eq!(found.len(), 1, "row {row:?} drawn once");
                    found[0]
                })
                .collect();
            for pair in rects.windows(2) {
                assert!(pair[0].bottom() <= pair[1].top() + 0.5, "the first line is on top: {rects:?}");
                assert!((pair[0].right() - pair[1].right()).abs() <= 1.0, "rows end at the same right edge: {rects:?}");
            }
        });
    }

    /// (check box rect, label text rect) for one check box labelled "Ruler".
    fn checkbox_rects(code: &str, ours: bool) -> (Rect, Rect) {
        with_lang(code, || {
            let mut h = egui_kittest::Harness::new_ui(|ui| {
                let mut on = true;
                if ours {
                    checkbox(ui, &mut on, "Ruler");
                } else {
                    ui.checkbox(&mut on, "Ruler");
                }
            });
            h.run();
            let node = h.get_by_label("Ruler").rect();
            let text = text_rects(h.output(), "Ruler");
            (node, text[0])
        })
    }

    #[test]
    fn rtl_checkbox_puts_the_box_right_of_its_label() {
        let (node, text) = checkbox_rects("ar", true);
        assert!(text.right() <= node.right() - 14.0 && text.left() <= node.left() + 1.0, "arabic: label {text:?} at the left of {node:?}");
        let (node, text) = checkbox_rects("en", true);
        assert!(text.left() >= node.left() + 14.0, "english: label {text:?} after the box in {node:?}");
        assert_eq!(checkbox_rects("en", true), checkbox_rects("en", false), "english: exactly the stock check box");
    }

    /// (radio button rect, label text rect) for one radio button labelled "Portrait".
    fn radio_rects(code: &str, ours: bool) -> (Rect, Rect) {
        with_lang(code, || {
            let mut h = egui_kittest::Harness::new_ui(|ui| {
                let mut v = 1;
                if ours {
                    radio_value(ui, &mut v, 1, "Portrait");
                } else {
                    ui.radio_value(&mut v, 1, "Portrait");
                }
            });
            h.run();
            let node = h.get_by_label("Portrait").rect();
            let text = text_rects(h.output(), "Portrait");
            (node, text[0])
        })
    }

    #[test]
    fn rtl_radio_puts_the_circle_right_of_its_label() {
        let (node, text) = radio_rects("ar", true);
        assert!(text.right() <= node.right() - 14.0 && text.left() <= node.left() + 1.0, "arabic: label {text:?} at the left of {node:?}");
        let (node, text) = radio_rects("en", true);
        assert!(text.left() >= node.left() + 14.0, "english: label {text:?} after the circle in {node:?}");
        assert_eq!(radio_rects("en", true), radio_rects("en", false), "english: exactly the stock radio button");
    }

    /// (sub-menu button rect, sub-menu item rect) with the sub-menu open.
    fn sub_menu_rects(code: &str) -> (Rect, Rect) {
        with_lang(code, || {
            let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 400.0)).build_ui(|ui| {
                ui.add_space(20.0);
                ui.horizontal(|ui| {
                    ui.add_space(300.0);
                    let r = ui.button("Open");
                    menu(&r).show(|ui| {
                        let _ = ui.button("First");
                        sub_menu(ui, "More", |ui| {
                            let _ = ui.button("Inner");
                        });
                    });
                });
            });
            h.run();
            h.get_by_label("Open").click();
            h.run_steps(3);
            assert!(egui::Popup::is_any_open(&h.ctx), "{code}: the menu opened");
            h.get_by_label_contains("More").hover();
            h.run_steps(4);
            (h.get_by_label_contains("More").rect(), h.get_by_label("Inner").rect())
        })
    }

    #[test]
    fn sub_menus_open_toward_the_end_side() {
        let (more, inner) = sub_menu_rects("ar");
        assert!(inner.right() <= more.left() + 1.0, "arabic: the sub-menu {inner:?} opens left of {more:?}");
        let (more, inner) = sub_menu_rects("en");
        assert!(inner.left() >= more.right() - 1.0, "english: the sub-menu {inner:?} opens right of {more:?}");
    }

    /// (caption rect, field rect) of each row of a fitted two-column grid in a dialog-like window,
    /// after a few frames: a short caption, then a long one.
    fn fitted_grid_rows(code: &str) -> Vec<(Rect, Rect)> {
        with_lang(code, || {
            let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 300.0)).build_ui(|ui| {
                let ctx = ui.ctx().clone();
                // As a dialog: an auto-sized window (its first frame is a sizing pass).
                egui::Window::new("w").resizable(false).collapsible(false).show(&ctx, |ui| {
                    ui.set_min_width(340.0);
                    column(ui, |ui| {
                        fitted(ui, "g", |ui| {
                            grid("g", 2).show(ui, |ui| {
                                for (caption, field) in [("A:", "short field"), ("A much longer caption:", "field two")] {
                                    grid_row(
                                        ui,
                                        [
                                            &mut |ui| {
                                                ui.label(caption);
                                            },
                                            &mut |ui| {
                                                let _ = ui.button(field);
                                            },
                                        ],
                                    );
                                }
                            });
                        });
                    });
                });
            });
            h.run_steps(6);
            [("A:", "short field"), ("A much longer caption:", "field two")]
                .iter()
                .map(|(c, f)| (h.get_by_label(c).rect(), h.get_by_label(f).rect()))
                .collect()
        })
    }

    #[test]
    fn rtl_grid_columns_fit_their_widest_cell() {
        let gap = egui::Style::default().spacing.item_spacing.x;
        for (caption, field) in fitted_grid_rows("ar") {
            // The field ends left of its caption's column, with no overlap and no wide gap.
            assert!(field.right() <= caption.left() + 0.5, "arabic: field {field:?} overlaps caption {caption:?}");
        }
        let rows = fitted_grid_rows("ar");
        let (long_caption, long_field) = rows[1];
        assert!(long_caption.left() - long_field.right() <= gap + 12.0, "arabic: wide gap {long_field:?} .. {long_caption:?}");
        let (short_caption, _) = rows[0];
        assert!((short_caption.right() - long_caption.right()).abs() <= 0.5, "arabic: captions share their right edge");
    }

    #[test]
    fn grid_row_keeps_english_rows_unchanged_and_rtl_rows_the_same_height() {
        let plain = grid_metrics("en", false);
        assert_eq!(grid_metrics("en", true), plain, "english: same y and grid height");
        let (y, height) = grid_metrics("ar", true);
        assert!((y - plain.0).abs() <= 1.0, "arabic y {y} vs english {}", plain.0);
        assert!((height - plain.1).abs() <= 1.0, "arabic height {height} vs {}", plain.1);
    }
}
