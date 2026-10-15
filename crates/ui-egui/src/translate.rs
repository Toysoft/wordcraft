//! Review › Translate: a popup under the selection with its translation from Google Cloud
//! Translation (v2), using the API key set in File › Options › Language. Nothing leaves the
//! machine until the user asks for a translation; Insert puts it in place of the selection
//! through `review.translateSelection`, which agents can also run with a translation of their own.
//! Without a key, each word is looked up in the user's StarDict dictionaries instead (`stardict`).

use std::sync::{Arc, Mutex};

use egui::{Pos2, Rect, vec2};
use serde_json::{Value, json};

use crate::WordApp;
use crate::i18n::{self, Lang};

const ENDPOINT: &str = "https://translation.googleapis.com/language/translate/v2";

/// The most text sent in one request (Google takes more; this keeps requests quick and cheap).
pub const MAX_CHARS: usize = 5000;

/// A request's answer, filled in by the network callback: `None` while waiting.
type Answer = Arc<Mutex<Option<Result<String, String>>>>;

/// The open popup.
pub struct Popup {
    /// The selected text when the popup opened, and where: selecting elsewhere closes the popup.
    source: String,
    sel: wordcraft_engine::Selection,
    /// The interface language code translated into.
    to: String,
    /// Where it opens: under the selection, or the middle of the page area.
    at: Option<Pos2>,
    answer: Option<Answer>,
    /// Without a key: each word with its dictionary entry, or `None` when no dictionary is installed.
    offline: Option<Vec<(String, Option<String>)>>,
}

impl Popup {
    pub fn new(app: &WordApp) -> Popup {
        let to = Some(app.ui.translate_to.as_str()).filter(|c| Lang::all().any(|l| l.code() == *c)).map_or(i18n::current().code(), |c| c);
        let source = app.session.selected_text();
        let offline = if app.ui.translate_key.trim().is_empty() { crate::stardict::look_up(&source) } else { None };
        Popup {
            offline,
            source,
            sel: app.session.sel.clone(),
            to: to.to_string(),
            at: app.canvas.mini_anchor.map(|r| r.left_bottom() + vec2(0.0, 8.0)),
            answer: None,
        }
    }
}

/// Google's code for an interface language.
fn google_code(lang: &str) -> &str {
    match lang {
        "zh-hans" => "zh-CN",
        "zh-hant" => "zh-TW",
        "pt-br" => "pt",
        "sr-latn" => "sr",
        other => other,
    }
}

fn request(key: &str, text: &str, to: &str) -> ehttp::Request {
    let body = json!({"q": text, "target": google_code(to), "format": "text"}).to_string().into_bytes();
    // The key goes in a header rather than the URL, so it stays out of proxy and server logs.
    ehttp::Request::new(ehttp::Method::POST, ENDPOINT, &[("Content-Type", "application/json"), ("X-Goog-Api-Key", key)]).with_body(body)
}

/// The translation in Google's answer, or its error message (Google's own, in English).
fn parse(status: u16, body: &[u8]) -> Result<String, String> {
    let v: Value = serde_json::from_slice(body).unwrap_or_default();
    if let Some(t) = v.pointer("/data/translations/0/translatedText").and_then(Value::as_str) {
        return Ok(t.to_string());
    }
    Err(v.pointer("/error/message").and_then(Value::as_str).map_or_else(|| format!("Google Translate: HTTP {status}"), str::to_string))
}

fn start(ctx: &egui::Context, key: &str, text: &str, to: &str) -> Answer {
    let answer = Answer::default();
    let (slot, ctx) = (answer.clone(), ctx.clone());
    ehttp::fetch(request(key, text, to), move |r| {
        if let Ok(mut s) = slot.lock() {
            *s = Some(r.and_then(|resp| parse(resp.status, &resp.bytes)));
        }
        ctx.request_repaint();
    });
    answer
}

/// Draws the popup while it is open.
pub fn show(app: &mut WordApp, ctx: &egui::Context, viewport: Rect) {
    let Some(mut p) = app.canvas.translate.take() else { return };
    if app.session.sel != p.sel {
        return;
    }
    let key = app.ui.translate_key.trim().to_string();
    let too_long = p.source.chars().count() > MAX_CHARS;
    if p.answer.is_none() && !key.is_empty() && !too_long {
        p.answer = Some(start(ctx, &key, &p.source, &p.to));
    }
    let mut open = true;
    let mut close = false;
    let at = p.at.unwrap_or(viewport.center() - vec2(180.0, 80.0));
    egui::Window::new(tl!("Translate"))
        .id(egui::Id::new("translate_popup"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(at)
        .constrain_to(viewport)
        .default_width(360.0)
        .show(ctx, |ui| {
            if !key.is_empty() {
            ui.horizontal(|ui| {
                ui.label(tl!("Language"));
                let name = Lang::all().find(|l| l.code() == p.to).map_or("", Lang::name);
                egui::ComboBox::from_id_salt("translate_to").selected_text(name).show_ui(ui, |ui| {
                    for l in Lang::all() {
                        if ui.selectable_label(l.code() == p.to, l.name()).clicked() && l.code() != p.to {
                            p.to = l.code().to_string();
                            app.ui.translate_to = p.to.clone();
                            p.answer = None;
                        }
                    }
                });
            });
            }
            ui.add_space(4.0);
            let result = p.answer.as_ref().and_then(|a| a.lock().ok().and_then(|s| s.clone()));
            let mut translation = None;
            let mut copy = None;
            if let (true, Some(words)) = (key.is_empty(), &p.offline) {
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for (word, entry) in words {
                        ui.label(egui::RichText::new(word).strong());
                        match entry {
                            Some(e) => ui.add(egui::Label::new(e).selectable(true).wrap()),
                            None => ui.weak("—"),
                        };
                        ui.add_space(4.0);
                    }
                });
                copy = Some(words.iter().map(|(w, e)| format!("{w}: {}", e.as_deref().unwrap_or("—"))).collect::<Vec<_>>().join("\n"));
            } else if key.is_empty() {
                ui.label(tl!(
                    "Add your Google Cloud Translation API key in File › Options › Language. The selected text is sent to Google to translate it. Or, to look words up offline, put a StarDict dictionary (.ifo, .idx and .dict files) in the dictionaries folder in File › Options › Proofing."
                ));
                if ui.button(tl!("Options…")).clicked() {
                    app.ui.options_pane = "language".into();
                    let _ = app.run("ui.backstage", json!({"value": true, "page": "options"}));
                    close = true;
                }
            } else if too_long {
                ui.label(i18n::fmt(
                    tl!("Select less text: Translate takes up to {count} characters at a time."),
                    &[("count", &MAX_CHARS.to_string())],
                ));
            } else {
                match result {
                    None => {
                        ui.spinner();
                    }
                    Some(Err(e)) => {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    Some(Ok(t)) => {
                        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                            ui.add(egui::Label::new(&t).selectable(true).wrap());
                        });
                        copy = Some(t.clone());
                        translation = Some(t);
                    }
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if let Some(t) = translation
                    && ui.add(crate::widgets::primary_button(ui, tl!("Insert"))).clicked()
                {
                    close = app.run("review.translateSelection", json!({"text": t})).is_ok();
                }
                if let Some(t) = copy
                    && ui.button(tl!("Copy")).clicked()
                {
                    ui.ctx().copy_text(t);
                }
                if ui.button(tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    if open && !close {
        app.canvas.translate = Some(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_translation_or_the_error() {
        let ok = r#"{"data":{"translations":[{"translatedText":"Xin chào","detectedSourceLanguage":"en"}]}}"#.as_bytes();
        assert_eq!(parse(200, ok), Ok("Xin chào".to_string()));
        let bad = br#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key."}}"#;
        assert_eq!(parse(400, bad), Err("API key not valid. Please pass a valid API key.".to_string()));
        assert_eq!(parse(502, b"<html>"), Err("Google Translate: HTTP 502".to_string()));
        assert!(parse(200, br#"{"data":{"translations":[]}}"#).is_err());
    }

    #[test]
    fn asks_google_in_its_language_codes_with_the_key_in_a_header() {
        let r = request("k3y", "Hello \"world\"", "zh-hans");
        assert_eq!(r.headers.get("X-Goog-Api-Key"), Some("k3y"));
        assert!(!r.url.contains("k3y"));
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(body, json!({"q": "Hello \"world\"", "target": "zh-CN", "format": "text"}));
        assert_eq!(google_code("vi"), "vi");
    }
}
