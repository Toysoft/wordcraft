//! Footnote and Endnote (References › Footnotes launcher, #385): where notes go, how they are
//! numbered, for the whole document or the caret's section; Convert; Insert with a custom mark.
//! The form ends in commands (`references.noteOptions`, `references.convertNotes`,
//! `references.footnote` / `references.endnote`), so agents get the same result without it.

use egui::{Ui, vec2};
use serde::Serialize;
use serde_json::{Value, json};

use crate::theme::semibold;
use crate::{WordApp, rtl};

/// Number formats notes offer: OOXML name and a sample (not translated).
const FORMATS: [(&str, &str); 5] = [
    ("decimal", "1, 2, 3, …"),
    ("lowerLetter", "a, b, c, …"),
    ("upperLetter", "A, B, C, …"),
    ("lowerRoman", "i, ii, iii, …"),
    ("upperRoman", "I, II, III, …"),
];

/// The dialog's fields; `[footnotes, endnotes]` where each kind has its own.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NoteForm {
    /// Which kind the format fields and Insert are for.
    pub endnote: bool,
    pub pos: [String; 2],
    pub fmt: [String; 2],
    pub start: [u32; 2],
    pub restart: [String; 2],
    /// A custom mark for Insert (empty: numbered).
    pub mark: String,
    /// Apply to the whole document (else the caret's section).
    pub whole: bool,
}

impl NoteForm {
    /// The options in effect at the caret.
    pub fn read(app: &mut WordApp) -> NoteForm {
        let v = app.session.run("references.noteOptions", &json!({})).unwrap_or_default();
        let s = |kind: &str, k: &str, d: &str| v.get(kind).and_then(|o| o.get(k)).and_then(Value::as_str).unwrap_or(d).to_string();
        let n = |kind: &str| v.get(kind).and_then(|o| o.get("numStart")).and_then(Value::as_u64).unwrap_or(1).clamp(1, 32_767) as u32;
        NoteForm {
            endnote: false,
            pos: [s("footnote", "pos", "pageBottom"), s("endnote", "pos", "docEnd")],
            fmt: [s("footnote", "numFmt", "decimal"), s("endnote", "numFmt", "lowerRoman")],
            start: [n("footnote"), n("endnote")],
            restart: [s("footnote", "numRestart", "continuous"), s("endnote", "numRestart", "continuous")],
            mark: String::new(),
            whole: true,
        }
    }

    /// `references.noteOptions` parameters for the chosen kind.
    pub fn params(&self) -> Value {
        let k = usize::from(self.endnote);
        let get = |a: &[String; 2]| a.get(k).cloned().unwrap_or_default();
        json!({
            "kind": if self.endnote { "endnote" } else { "footnote" },
            "pos": get(&self.pos),
            "numFmt": get(&self.fmt),
            "numStart": self.start.get(k).copied().unwrap_or(1),
            "numRestart": get(&self.restart),
            "scope": if self.whole { "document" } else { "section" },
        })
    }
}

/// A choice from `options` ((value, label)) in a drop-down; `tr` translates the labels.
fn choice(ui: &mut Ui, id: &str, value: &mut String, options: &[(&str, &str)], tr: bool) {
    let show = |l: &str| if tr { tl!(l).to_string() } else { l.to_string() };
    let label = options.iter().find(|(v, _)| *v == value.as_str()).map(|(_, l)| show(l)).unwrap_or_default();
    egui::ComboBox::from_id_salt(id).selected_text(label).width(170.0).show_ui(ui, |ui| {
        for (v, l) in options {
            ui.selectable_value(value, v.to_string(), show(l));
        }
    });
}

/// Footnote and Endnote. Returns true to close.
pub fn note_options(app: &mut WordApp, ui: &mut Ui, f: &mut NoteForm) -> bool {
    let heading = |ui: &mut Ui, s: &str| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!(s)).font(semibold(12.5)));
    };
    heading(ui, "Location");
    let mut convert = None;
    rtl::fitted(ui, "notes_location", |ui| {
        rtl::grid("notes_location", 2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            let NoteForm { endnote, pos, .. } = f;
            let [fpos, epos] = pos;
            let on_endnotes = *endnote;
            rtl::grid_row(
                ui,
                [
                    &mut |ui: &mut Ui| {
                        rtl::radio_value(ui, endnote, false, tl!("Footnotes"));
                    },
                    &mut |ui: &mut Ui| {
                        ui.add_enabled_ui(!on_endnotes, |ui| {
                            choice(ui, "notes_fpos", fpos, &[("pageBottom", "Bottom of Page"), ("beneathText", "Below text")], true);
                        });
                    },
                ],
            );
            rtl::grid_row(
                ui,
                [
                    &mut |ui: &mut Ui| {
                        rtl::radio_value(ui, endnote, true, tl!("Endnotes"));
                    },
                    &mut |ui: &mut Ui| {
                        ui.add_enabled_ui(on_endnotes, |ui| {
                            choice(ui, "notes_epos", epos, &[("sectEnd", "End of section"), ("docEnd", "End of document")], true);
                        });
                    },
                ],
            );
            rtl::grid_row(
                ui,
                [
                    &mut |ui: &mut Ui| {
                        ui.label("");
                    },
                    &mut |ui: &mut Ui| {
                        // The ellipsis ends "Convert…" in reading order (its left under RTL).
                        let label = crate::i18n::suffixed(tl!("Convert"), "…");
                        rtl::sub_menu(ui, label, |ui| {
                            for (to, l) in
                                [("endnote", "Footnotes to endnotes"), ("footnote", "Endnotes to footnotes"), ("swap", "Swap footnotes and endnotes")]
                            {
                                if ui.button(tl!(l)).clicked() {
                                    convert = Some(to);
                                    ui.close();
                                }
                            }
                        });
                    },
                ],
            );
        });
    });
    if let Some(to) = convert
        && let Err(e) = app.run("references.convertNotes", json!({ "to": to }))
    {
        app.status(e);
    }
    heading(ui, "Format");
    let k = usize::from(f.endnote);
    rtl::fitted(ui, "notes_format", |ui| {
        rtl::grid("notes_format", 2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            let row = |ui: &mut Ui, caption: &str, field: &mut dyn FnMut(&mut Ui)| {
                rtl::grid_row(ui, [&mut |ui: &mut Ui| crate::dialogs::label(ui, caption), field]);
            };
            row(ui, "Number format", &mut |ui: &mut Ui| {
                if let Some(v) = f.fmt.get_mut(k) {
                    choice(ui, "notes_fmt", v, &FORMATS, false);
                }
            });
            row(ui, "Custom mark:", &mut |ui: &mut Ui| {
                crate::dialogs::cell_width(ui, 60.0);
                ui.add(egui::TextEdit::singleline(&mut f.mark).desired_width(60.0).char_limit(10));
            });
            row(ui, "Start at:", &mut |ui: &mut Ui| {
                if let Some(v) = f.start.get_mut(k) {
                    ui.add(egui::DragValue::new(v).range(1..=32_767).speed(0.2));
                }
            });
            let endnote = f.endnote;
            row(ui, "Numbering", &mut |ui: &mut Ui| {
                if let Some(v) = f.restart.get_mut(k) {
                    let each_page: &[(&str, &str)] = if endnote { &[] } else { &[("eachPage", "Restart Each Page")] };
                    let options: Vec<(&str, &str)> = [("continuous", "Continuous numbering"), ("eachSect", "Restart Each Section")]
                        .into_iter()
                        .chain(each_page.iter().copied())
                        .collect();
                    choice(ui, "notes_restart", v, &options, true);
                }
            });
            row(ui, "Apply to:", &mut |ui: &mut Ui| {
                let mut scope = if f.whole { "document" } else { "section" }.to_string();
                choice(ui, "notes_scope", &mut scope, &[("document", "Whole document"), ("section", "This section")], true);
                f.whole = scope == "document";
            });
        });
    });
    // Insert / Apply / Cancel.
    let (mut insert, mut apply, mut cancel) = (false, false, false);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.with_layout(crate::rtl::end_layout(egui::Align::Center), |ui| {
            cancel = ui.button(tl!("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            apply = ui.button(tl!("Apply")).clicked();
            let label = if f.endnote { "Insert Endnote" } else { "Insert Footnote" };
            insert = ui.add(crate::widgets::primary_button(ui, tl!(label))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
        });
    });
    if insert || apply {
        if let Err(e) = app.run("references.noteOptions", f.params()) {
            app.status(e);
            return false;
        }
        if insert {
            let id = if f.endnote { "references.endnote" } else { "references.footnote" };
            if let Err(e) = app.run(id, json!({ "mark": f.mark.trim() })) {
                app.status(e);
            }
        }
    }
    insert || apply || cancel
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wordcraft_engine::Session;

    use crate::dialogs::Dialog;
    use crate::{Services, WordApp};

    /// #385: the Footnotes group's launcher opens Footnote and Endnote; its settings apply to the
    /// caret's section. Scripts get the options, never the dialog.
    #[test]
    fn footnote_and_endnote_opens_from_the_launcher_and_applies() {
        let mut a = WordApp::new(Session::new(wordcraft_doc::Document::new()), Services::default());
        assert_eq!(a.run("references.noteOptions", json!({})).unwrap(), json!({"pending": "noteOptions"}));
        let Some(Dialog::NoteOptions { form }) = &mut a.dialog else { panic!("dialog") };
        assert_eq!((form.pos[0].as_str(), form.fmt[1].as_str(), form.start[0]), ("pageBottom", "lowerRoman", 1));
        form.pos[0] = "beneathText".into();
        form.restart[0] = "eachPage".into();
        form.whole = false;
        let params = form.params();
        a.dialog = None;
        a.run("references.noteOptions", params).unwrap();
        let o = a.session.doc.note_options(&a.session.doc.last_section, false);
        assert_eq!(o.pos, wordcraft_doc::section::NotePos::BeneathText);
        assert!(a.session.doc.settings.footnote_pr.is_empty(), "this section only");
        let got = a.execute("references.noteOptions", json!({})).unwrap();
        assert_eq!(got["footnote"]["numRestart"], "eachPage");
        assert!(a.dialog.is_none());
    }
}
