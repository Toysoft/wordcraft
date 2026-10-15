//! Dictate (Home › Voice) and Translate Document (Review), with local models: what the window
//! does around the `tools.dictate` and `review.translate` commands.
//!
//! - **Dictate** records the microphone (the host's [`Microphone`], `Services::microphone`) and
//!   hands the audio to a worker thread, which loads the Whisper model, cuts the audio into
//!   phrases at the speaker's pauses and transcribes each one. Every phrase comes back to the UI
//!   thread and goes in at the caret through `tools.dictate {"text"}` — one undo step each.
//!   Pressing Dictate again (or Stop) ends the recording; the phrase being spoken is still written.
//! - **Translate Document** asks what to translate into, works out the texts with the engine's
//!   [`wordcraft_engine::translate::Plan`], and translates them on a worker thread with a progress
//!   window and Cancel. The finished translations go to `review.translate {"translations"}`,
//!   which puts them in a new document (or in place) in one undoable step.
//!
//! Nothing runs on the UI thread but bookkeeping, and nothing leaves the computer. The web build
//! has no models and says so.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use egui::Ui;
use serde::Serialize;
use serde_json::{Value, json};

use crate::WordApp;

/// A live microphone recording from the host's audio API (desktop). Dropping it stops recording.
pub trait Microphone {
    /// Samples per second.
    fn rate(&self) -> u32;
    /// Channels, interleaved in what [`Microphone::take`] returns.
    fn channels(&self) -> u16;
    /// The samples recorded since the last call (-1.0..=1.0).
    fn take(&mut self) -> Vec<f32>;
    /// Why recording stopped, if it did (the device went away).
    fn error(&self) -> Option<String>;
}

/// Opens the default microphone: the host's `Services::microphone`.
pub type OpenMicrophone = Box<dyn Fn() -> Result<Box<dyn Microphone>, String>>;

/// The languages offered for dictation and translation: (code, English name, translated for
/// display). The commands take any code the model knows.
pub const LANGUAGES: [(&str, &str); 28] = [
    ("ar", "Arabic"),
    ("bg", "Bulgarian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sv", "Swedish"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("vi", "Vietnamese"),
    ("zh", "Chinese"),
];

/// A language's name in the interface language (the code for one not listed).
pub fn language_name(code: &str) -> String {
    let c = wordcraft_ml::lang::normalize(code);
    LANGUAGES.iter().find(|(k, _)| *k == c).map_or(c.clone(), |(_, n)| tl!(n).to_string())
}

/// The listed languages sorted by their names in the interface language.
fn sorted_languages() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = LANGUAGES.iter().map(|(c, _)| (*c, language_name(c))).collect();
    v.sort_by_key(|(_, n)| n.to_lowercase());
    v
}

/// A language picker; `""` is "Detect automatically" when `auto` is true.
fn language_combo(ui: &mut Ui, id: &str, value: &mut String, auto: bool) -> bool {
    let shown = if value.is_empty() { tl!("Detect automatically").to_string() } else { language_name(value) };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id).selected_text(shown).width(220.0).show_ui(ui, |ui| {
        if auto {
            changed |= ui.selectable_value(value, String::new(), tl!("Detect automatically")).changed();
        }
        for (code, name) in sorted_languages() {
            changed |= ui.selectable_value(value, code.to_string(), name).changed();
        }
    });
    changed
}

// ---------------------------------------------------------------------------------------------
// Dictate

/// What the dictation worker tells the window (the web build has no worker).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Heard {
    /// The model is loaded: listening.
    Ready,
    /// Whether a phrase is being spoken.
    Hearing(bool),
    /// Whether a phrase is being written down.
    Busy(bool),
    /// A phrase's text.
    Text(String),
    Error(String),
    /// The worker has finished.
    Done,
}

/// Dictation in progress.
struct Running {
    /// Recording; `None` once stopped (the last phrase may still be being written down).
    mic: Option<Box<dyn Microphone>>,
    audio: Option<Sender<Vec<f32>>>,
    events: Receiver<Heard>,
    cancel: Arc<AtomicBool>,
    /// What the worker hears (a code; `None`: detect), changed from the window.
    language: Arc<Mutex<Option<String>>>,
    loading: bool,
    hearing: bool,
    busy: bool,
}

/// Dictate's state in the window.
#[derive(Default)]
pub struct Dictation {
    run: Option<Running>,
}

impl Dictation {
    /// Whether Dictate is on (the ribbon button shows pressed).
    pub fn active(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.mic.is_some())
    }
}

/// The Dictate button: start listening, or stop.
pub fn toggle(app: &mut WordApp) -> Result<Value, String> {
    if app.dictation.active() {
        stop(app);
        return Ok(json!({"dictating": false}));
    }
    if app.dictation.run.is_some() {
        // Still writing down the last phrase of the previous recording.
        let msg = tl!("Still writing down what you said…").to_string();
        app.status(msg.clone());
        return Err(msg);
    }
    match start(app) {
        Ok(v) => Ok(v),
        Err(e) => {
            app.status(e.clone());
            Err(e)
        }
    }
}

/// Stop recording; what was said so far is still written.
pub fn stop(app: &mut WordApp) {
    if let Some(r) = app.dictation.run.as_mut() {
        r.mic = None;
        r.audio = None;
    }
}

/// Stop at once, dropping what hasn't been written yet (the document is closing).
pub fn cancel(app: &mut WordApp) {
    if let Some(r) = app.dictation.run.take() {
        r.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(target_arch = "wasm32")]
fn start(_app: &mut WordApp) -> Result<Value, String> {
    Err(tl!("Dictate isn't available in the web version of WordCraft.").to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn start(app: &mut WordApp) -> Result<Value, String> {
    use std::path::PathBuf;
    use std::sync::mpsc::channel;
    let Some(model) = app.session.prefs.speech_model.clone() else {
        // Show where the setting is.
        let _ = app.run("file.options", json!({"pane": "language"}));
        return Err(wordcraft_engine::cmd::local_ai::NO_SPEECH_MODEL.to_string());
    };
    let model = PathBuf::from(model);
    wordcraft_ml::models::check_speech_model(&model).map_err(|e| e.to_string())?;
    let Some(open) = app.services.microphone.as_ref() else {
        return Err(tl!("No microphone is available.").to_string());
    };
    let mic = open().map_err(|e| crate::i18n::fmt(tl!("Couldn't open the microphone: {error}"), &[("error", &e)]))?;
    let (rate, channels) = (mic.rate(), mic.channels());
    let (audio_tx, audio_rx) = channel::<Vec<f32>>();
    let (events_tx, events_rx) = channel::<Heard>();
    let cancel = Arc::new(AtomicBool::new(false));
    let language = Arc::new(Mutex::new(app.session.prefs.dictation_language.clone()));
    let (c, l, wake) = (cancel.clone(), language.clone(), app.ctx.clone());
    std::thread::Builder::new()
        .name("dictation".into())
        .spawn(move || dictation_worker(&model, rate, channels, &audio_rx, &events_tx, &c, &l, wake.as_ref()))
        .map_err(|e| format!("could not start dictation: {e}"))?;
    app.dictation.run =
        Some(Running { mic: Some(mic), audio: Some(audio_tx), events: events_rx, cancel, language, loading: true, hearing: false, busy: false });
    app.status(tl!("Loading the speech model…"));
    Ok(json!({"dictating": true}))
}

/// The worker: load the model, then cut the audio it is sent into phrases and transcribe them,
/// until the sender is dropped (the last phrase is still transcribed) or `cancel` is set.
#[cfg(not(target_arch = "wasm32"))]
fn dictation_worker(
    model: &std::path::Path,
    rate: u32,
    channels: u16,
    audio: &Receiver<Vec<f32>>,
    events: &Sender<Heard>,
    cancel: &AtomicBool,
    language: &Mutex<Option<String>>,
    wake: Option<&egui::Context>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    let send = |e: Heard| {
        let _ = events.send(e);
        if let Some(c) = wake {
            c.request_repaint();
        }
    };
    let mut speech = match wordcraft_ml::SpeechModel::load(model) {
        Ok(m) => m,
        Err(e) => {
            send(Heard::Error(e.to_string()));
            send(Heard::Done);
            return;
        }
    };
    send(Heard::Ready);
    let mut stream = wordcraft_ml::audio::Stream::new(rate, channels);
    let mut phrases = wordcraft_ml::audio::Segmenter::new();
    let mut hearing = false;
    let mut write_down = |phrase: Vec<f32>| {
        send(Heard::Busy(true));
        let lang = language.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        match speech.transcribe(&phrase, lang.as_deref(), cancel) {
            Ok(t) if !t.text.is_empty() => send(Heard::Text(t.text)),
            Ok(_) | Err(wordcraft_ml::Error::Cancelled) => {}
            Err(e) => send(Heard::Error(e.to_string())),
        }
        send(Heard::Busy(false));
    };
    while !cancel.load(Ordering::Relaxed) {
        match audio.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(chunk) => {
                let done = phrases.push(&stream.push(&chunk));
                if phrases.hearing_speech() != hearing {
                    hearing = phrases.hearing_speech();
                    send(Heard::Hearing(hearing));
                }
                for p in done {
                    write_down(p);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let mut rest = phrases.push(&stream.finish());
                rest.extend(phrases.flush());
                for p in rest {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    write_down(p);
                }
                break;
            }
        }
    }
    send(Heard::Done);
}

/// Each frame: move the recorded audio to the worker, and put what it heard in the document.
pub fn poll(app: &mut WordApp, ctx: &egui::Context) {
    let Some(r) = app.dictation.run.as_mut() else { return };
    let mut texts = Vec::new();
    let mut errors = Vec::new();
    if let Some(mic) = r.mic.as_mut() {
        let samples = mic.take();
        if !samples.is_empty()
            && let Some(tx) = &r.audio
        {
            let _ = tx.send(samples);
        }
        if let Some(e) = mic.error() {
            errors.push(crate::i18n::fmt(tl!("The microphone stopped: {error}"), &[("error", &e)]));
            r.mic = None;
            r.audio = None;
        }
    }
    let mut done = false;
    loop {
        match r.events.try_recv() {
            Ok(Heard::Ready) => r.loading = false,
            Ok(Heard::Hearing(b)) => r.hearing = b,
            Ok(Heard::Busy(b)) => r.busy = b,
            Ok(Heard::Text(t)) => texts.push(t),
            Ok(Heard::Error(e)) => {
                // A model that didn't load: nothing more will be heard.
                if r.loading {
                    r.mic = None;
                    r.audio = None;
                }
                errors.push(e);
            }
            Ok(Heard::Done) | Err(TryRecvError::Disconnected) => {
                done = true;
                break;
            }
            Err(TryRecvError::Empty) => break,
        }
    }
    if done {
        app.dictation.run = None;
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    for t in texts {
        if let Err(e) = app.run("tools.dictate", json!({"text": t})) {
            errors.push(e);
        }
    }
    if let Some(e) = errors.pop() {
        app.status(e);
    }
}

/// The Dictation window: what is happening, the language heard, and Stop.
pub fn show(app: &mut WordApp, ctx: &egui::Context) {
    let Some(r) = app.dictation.run.as_ref() else { return };
    let t = crate::theme::Tokens::get(ctx);
    let what = if r.loading {
        tl!("Loading the speech model…")
    } else if r.busy || r.mic.is_none() {
        tl!("Writing down what you said…")
    } else if r.hearing {
        tl!("Listening…")
    } else {
        tl!("Speak now: text appears at the cursor when you pause.")
    };
    let hearing = r.hearing;
    let recording = r.mic.is_some();
    let mut lang = r.language.lock().map(|l| l.clone().unwrap_or_default()).unwrap_or_default();
    let mut open = true;
    let mut stop_now = false;
    let right_top =
        app.canvas.canvas_rect.map_or(ctx.content_rect().right_top() + egui::vec2(-24.0, 160.0), |r| r.right_top() + egui::vec2(-24.0, 16.0));
    egui::Window::new(tl!("Dictation"))
        .id(egui::Id::new("dictation-window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(right_top)
        .show(ctx, |ui| {
            ui.set_width(260.0);
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                let colour = if hearing { t.red } else { t.text_dim };
                ui.painter().circle_filled(dot.center(), if hearing { 5.5 } else { 4.0 }, colour);
                ui.label(what);
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(tl!("Language:"));
                if language_combo(ui, "dictation-language", &mut lang, true)
                    && let Some(r) = app.dictation.run.as_ref()
                    && let Ok(mut l) = r.language.lock()
                {
                    *l = Some(lang.clone()).filter(|c| !c.is_empty());
                }
            });
            ui.add_space(6.0);
            if ui.add_enabled(recording, egui::Button::new(tl!("Stop"))).clicked() {
                stop_now = true;
            }
        });
    if !open || stop_now {
        stop(app);
    }
}

// ---------------------------------------------------------------------------------------------
// Translate Document

/// The Translate Document dialog's fields.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateForm {
    /// Target language code.
    pub to: String,
    /// Source language code; empty: detect.
    pub from: String,
    /// Only the selected paragraphs (in place).
    pub selection: bool,
    /// The whole document into a new, untitled document (otherwise in place).
    pub new_document: bool,
    pub has_selection: bool,
    /// Why the last Translate didn't start.
    pub message: String,
    /// The translation models in the folder set in Options (looked up when the dialog opens).
    #[serde(skip)]
    models: Vec<wordcraft_ml::models::TranslationModel>,
    /// The language the text seems to be in.
    guessed: Option<String>,
}

impl TranslateForm {
    pub fn new(app: &WordApp) -> TranslateForm {
        let s = &app.session;
        let models =
            s.prefs.translation_models.as_deref().map(|f| wordcraft_ml::models::scan_translation_models(std::path::Path::new(f))).unwrap_or_default();
        let plan = wordcraft_engine::translate::Plan::new(&s.doc, &wordcraft_engine::translate::Scope::Document);
        let guessed = wordcraft_engine::cmd::local_ai::guess_source(s, &plan);
        let ui_lang = wordcraft_ml::lang::normalize(crate::i18n::current().code());
        // Into a language a model goes to from the text's language; else the interface's; else English.
        let to = models
            .iter()
            .find(|m| guessed.as_deref().is_none_or(|g| g == m.from) && !m.multi_target() && Some(&m.to) != guessed.as_ref())
            .map(|m| m.to.clone())
            .or_else(|| (LANGUAGES.iter().any(|(c, _)| *c == ui_lang) && Some(&ui_lang) != guessed.as_ref()).then(|| ui_lang.clone()))
            .unwrap_or_else(|| if guessed.as_deref() == Some("en") { "he".into() } else { "en".into() });
        TranslateForm {
            to,
            from: String::new(),
            selection: false,
            new_document: true,
            has_selection: !s.sel.is_collapsed(),
            message: String::new(),
            models,
            guessed,
        }
    }

    /// The source language: chosen, or guessed.
    fn source(&self) -> Option<String> {
        if self.from.is_empty() { self.guessed.clone() } else { Some(self.from.clone()) }
    }

    /// The model the current choice would use.
    fn model(&self) -> Option<&wordcraft_ml::models::TranslationModel> {
        wordcraft_ml::models::pick(&self.models, self.source().as_deref(), &self.to)
    }
}

/// Draws the Translate Document dialog; returns true to close it.
pub fn translate_ui(app: &mut WordApp, ui: &mut Ui, f: &mut TranslateForm) -> bool {
    let t = crate::theme::Tokens::get(ui.ctx());
    egui::Grid::new("translate-languages").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(tl!("Translate into:"));
        language_combo(ui, "translate-to", &mut f.to, false);
        ui.end_row();
        ui.label(tl!("From:"));
        language_combo(ui, "translate-from", &mut f.from, true);
        ui.end_row();
    });
    ui.add_space(6.0);
    if f.has_selection {
        ui.radio_value(&mut f.selection, false, tl!("The whole document"));
        ui.radio_value(&mut f.selection, true, tl!("The selected paragraphs"));
        ui.add_space(4.0);
    }
    if !f.selection {
        ui.label(tl!("Put the translation in:"));
        ui.radio_value(&mut f.new_document, true, tl!("A new document"));
        ui.radio_value(&mut f.new_document, false, tl!("This document"));
    }
    ui.add_space(6.0);
    let model = f.model().map(|m| m.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    match &model {
        Some(name) => {
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Model: {name}"), &[("name", name)])).small().color(t.text_dim));
        }
        None => {
            ui.label(egui::RichText::new(tl!("No translation model for this language pair.")).small().color(t.red));
            ui.label(egui::RichText::new(tl!("Choose a folder of translation models in File › Options › Language.")).small().color(t.text_dim));
        }
    }
    ui.label(egui::RichText::new(tl!("Formatting, tables, pictures and styles are kept. Undo brings the original back.")).small().color(t.text_dim));
    if !f.message.is_empty() {
        ui.label(egui::RichText::new(f.message.as_str()).small().color(ui.visuals().error_fg_color));
    }
    let same = f.source().is_some_and(|s| s == f.to);
    let mut close = false;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tl!("Cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
            let busy = app.translation.is_some();
            let go = ui.add_enabled(model.is_some() && !same && !busy, crate::widgets::primary_button(ui, tl!("Translate")));
            if go.clicked() {
                match start_translation(app, f) {
                    Ok(()) => close = true,
                    Err(e) => f.message = e,
                }
            }
        });
    });
    close
}

/// What the translation worker tells the window (the web build has no worker).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Progress {
    Loaded,
    /// One more text translated.
    One,
    Finished(HashMap<String, String>),
    Failed(String),
}

/// A translation in progress.
pub struct TranslationJob {
    events: Receiver<Progress>,
    cancel: Arc<AtomicBool>,
    /// `review.translate`'s parameters, but the translations.
    params: Value,
    total: usize,
    done: usize,
    loading: bool,
    document: u64,
}

#[cfg(target_arch = "wasm32")]
fn start_translation(_app: &mut WordApp, _f: &TranslateForm) -> Result<(), String> {
    Err(tl!("Translate isn't available in the web version of WordCraft.").to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_translation(app: &mut WordApp, f: &TranslateForm) -> Result<(), String> {
    use std::sync::mpsc::channel;
    use wordcraft_engine::cmd::local_ai;
    let selection = f.selection && f.has_selection;
    let plan = wordcraft_engine::translate::Plan::new(&app.session.doc, &local_ai::translate_scope(&app.session, selection));
    let texts = plan.texts();
    if texts.is_empty() {
        return Err(tl!("There is no text to translate.").to_string());
    }
    let from = if f.from.is_empty() { local_ai::guess_source(&app.session, &plan) } else { Some(f.from.clone()) };
    let dir = local_ai::find_translation_model(&app.session, None, from.as_deref(), &f.to).map_err(|e| e.to_string())?;
    let params = json!({
        "to": f.to,
        "from": from,
        "scope": if selection { "selection" } else { "document" },
        "showIn": if selection || !f.new_document { "original" } else { "new" },
    });
    let (tx, rx) = channel::<Progress>();
    let cancel = Arc::new(AtomicBool::new(false));
    let (c, to, wake, total) = (cancel.clone(), f.to.clone(), app.ctx.clone(), texts.len());
    std::thread::Builder::new()
        .name("translation".into())
        .spawn(move || {
            let send = |p: Progress| {
                let _ = tx.send(p);
                if let Some(w) = &wake {
                    w.request_repaint();
                }
            };
            let mut model = match wordcraft_ml::Translator::load(&dir, Some(&to)) {
                Ok(m) => m,
                Err(e) => return send(Progress::Failed(e.to_string())),
            };
            send(Progress::Loaded);
            let mut out = HashMap::with_capacity(texts.len());
            for text in texts {
                match model.translate(&text, &c) {
                    Ok(t) => {
                        out.insert(text, t);
                        send(Progress::One);
                    }
                    Err(wordcraft_ml::Error::Cancelled) => return,
                    Err(e) => return send(Progress::Failed(e.to_string())),
                }
            }
            send(Progress::Finished(out));
        })
        .map_err(|e| format!("could not start translating: {e}"))?;
    app.translation = Some(TranslationJob { events: rx, cancel, params, total, done: 0, loading: true, document: app.session.document_id() });
    Ok(())
}

/// Each frame: follow the translation, and put it in the document when it is done.
pub fn poll_translation(app: &mut WordApp, ctx: &egui::Context) {
    let Some(job) = app.translation.as_mut() else { return };
    let mut finished = None;
    let mut failed = None;
    loop {
        match job.events.try_recv() {
            Ok(Progress::Loaded) => job.loading = false,
            Ok(Progress::One) => job.done += 1,
            Ok(Progress::Finished(m)) => {
                finished = Some(m);
                break;
            }
            Ok(Progress::Failed(e)) => {
                failed = Some(e);
                break;
            }
            Err(TryRecvError::Empty) => break,
            // Cancelled: the worker stopped without a word.
            Err(TryRecvError::Disconnected) => {
                failed = Some(String::new());
                break;
            }
        }
    }
    if finished.is_none() && failed.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    let Some(job) = app.translation.take() else { return };
    if let Some(e) = failed.filter(|e| !e.is_empty()) {
        app.status(e);
        return;
    }
    let Some(map) = finished else { return };
    if job.document != app.session.document_id() {
        app.status(tl!("The translation was for a document that is no longer open."));
        return;
    }
    let mut params = job.params;
    params["translations"] = json!(map);
    match app.run("review.translate", params) {
        Ok(v) if v.get("pending").is_some() => {}
        Ok(v) => {
            let n = v.get("paragraphs").and_then(Value::as_u64).unwrap_or(0).to_string();
            app.status(crate::i18n::fmt(tl!("Translated paragraphs: {count}"), &[("count", &n)]));
        }
        Err(e) => app.status(e),
    }
}

/// The progress window while translating, with Cancel.
pub fn show_translation(app: &mut WordApp, ctx: &egui::Context) {
    let Some(job) = app.translation.as_ref() else { return };
    let line = if job.loading {
        tl!("Loading the translation model…").to_string()
    } else {
        crate::i18n::fmt(tl!("Translating… {done} of {total}"), &[("done", &job.done.to_string()), ("total", &job.total.to_string())])
    };
    let fraction = if job.total == 0 { 0.0 } else { job.done as f32 / job.total as f32 };
    let mut cancel = false;
    egui::Window::new(tl!("Translate Document"))
        .id(egui::Id::new("translation-progress"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(line);
            ui.add(egui::ProgressBar::new(fraction).animate(job.loading));
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tl!("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
    if cancel && let Some(job) = app.translation.take() {
        job.cancel.store(true, Ordering::Relaxed);
        app.status(tl!("Translation cancelled."));
    }
}

// ---------------------------------------------------------------------------------------------
// File › Options › Language

/// The model settings: the speech model folder, the language Dictate hears, and the folder of
/// translation models, each with Browse… (pick any file in the folder).
pub fn model_settings(app: &mut WordApp, ui: &mut Ui) {
    folder_row(app, ui, "speechModel", tl!("Speech model folder (Whisper):"));
    ui.horizontal(|ui| {
        ui.label(tl!("Dictation language:"));
        let mut lang = app.session.prefs.dictation_language.clone().unwrap_or_default();
        if language_combo(ui, "options-dictation-language", &mut lang, true) {
            let value = if lang.is_empty() { "auto".to_string() } else { lang };
            if let Err(e) = app.run("file.options", json!({"dictationLanguage": value})) {
                app.status(e);
            }
        }
    });
    ui.add_space(4.0);
    folder_row(app, ui, "translationModels", tl!("Translation models folder (OPUS-MT):"));
    if let Some(folder) = app.session.prefs.translation_models.clone() {
        // Looked up again only when the folder changes.
        let id = egui::Id::new("options-translation-models-found");
        let found = match ui.data(|d| d.get_temp::<(String, Vec<String>)>(id)).filter(|(f, _)| *f == folder) {
            Some((_, v)) => v,
            None => {
                let v: Vec<String> = wordcraft_ml::models::scan_translation_models(std::path::Path::new(&folder))
                    .iter()
                    .map(|m| format!("{} → {}", language_name(&m.from), language_name(&m.to)))
                    .collect();
                ui.data_mut(|d| d.insert_temp(id, (folder.clone(), v.clone())));
                v
            }
        };
        let line = if found.is_empty() {
            tl!("No translation models found in this folder.").to_string()
        } else {
            crate::i18n::fmt(tl!("Language pairs: {pairs}"), &[("pairs", &found.join(", "))])
        };
        ui.label(egui::RichText::new(line).small().weak());
    }
}

/// A folder setting with Apply, Browse… and Clear.
fn folder_row(app: &mut WordApp, ui: &mut Ui, key: &'static str, label: &str) {
    let saved = match key {
        "speechModel" => app.session.prefs.speech_model.clone(),
        _ => app.session.prefs.translation_models.clone(),
    }
    .unwrap_or_default();
    let id = egui::Id::new(("options-folder", key));
    let mut folder = ui.data(|d| d.get_temp::<(String, String)>(id)).filter(|(s, _)| *s == saved).map_or_else(|| saved.clone(), |(_, f)| f);
    ui.label(label);
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut folder).desired_width(300.0).hint_text(tl!("Not set")));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let changed = folder.trim() != saved;
        if ui.add_enabled(changed, egui::Button::new(tl!("Apply"))).clicked() || (enter && changed) {
            let value = if folder.trim().is_empty() { "auto" } else { folder.trim() };
            if let Err(e) = app.run("file.options", json!({key: value})) {
                app.status(e);
            }
        }
        if ui.button(tl!("Browse…")).clicked() {
            app.ask_file(crate::FileDialogRequest::Open { purpose: "model".into() }, crate::file_dialogs::AfterPick::ModelFolder { key });
        }
        if ui.add_enabled(!saved.is_empty(), egui::Button::new(tl!("Clear"))).clicked() && app.run("file.options", json!({key: "auto"})).is_ok() {
            folder.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, (saved, folder)));
}

/// The folder to save for a file picked with Browse…: the file's folder; for translation models,
/// the folder above a model's own, so the other language pairs beside it are found too.
pub fn picked_model_folder(key: &str, picked: &str) -> String {
    let p = std::path::Path::new(picked);
    let dir = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
    let dir = if key == "translationModels" && wordcraft_ml::models::is_translation_model(dir) { dir.parent().unwrap_or(dir) } else { dir };
    dir.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> WordApp {
        WordApp::new(wordcraft_engine::Session::new(wordcraft_engine::doc::Document::from_text("Good morning.")), Default::default())
    }

    #[test]
    fn dictate_without_a_model_says_so_and_opens_the_setting() {
        let mut a = app();
        let e = a.run("tools.dictate", json!({})).unwrap_err();
        assert!(e.contains("No speech model"), "{e}");
        assert!(!a.dictation.active());
        // Scripts still insert text directly.
        a.run("tools.dictate", json!({"text": "Hello."})).unwrap();
        assert!(a.session.doc.body[0].as_para().unwrap().text.contains("Hello."));
    }

    #[test]
    fn dictate_without_a_microphone_says_so() {
        let mut a = app();
        let dir = std::env::temp_dir().join(format!("wc-ui-speech-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in wordcraft_ml::models::SPEECH_FILES {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        a.run("file.options", json!({"speechModel": dir.to_string_lossy()})).unwrap();
        let e = a.run("tools.dictate", json!({})).unwrap_err();
        assert!(e.contains("microphone"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A fake microphone that plays back silence then reports a failure.
    struct FakeMic(usize);

    impl Microphone for FakeMic {
        fn rate(&self) -> u32 {
            48_000
        }
        fn channels(&self) -> u16 {
            2
        }
        fn take(&mut self) -> Vec<f32> {
            self.0 += 1;
            vec![0.0; 9600]
        }
        fn error(&self) -> Option<String> {
            (self.0 > 3).then(|| "unplugged".to_string())
        }
    }

    #[test]
    fn a_broken_model_ends_dictation_with_its_error() {
        let mut a = app();
        let dir = std::env::temp_dir().join(format!("wc-ui-speech-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for f in wordcraft_ml::models::SPEECH_FILES {
            std::fs::write(dir.join(f), "not a model").unwrap();
        }
        a.run("file.options", json!({"speechModel": dir.to_string_lossy()})).unwrap();
        a.services.microphone = Some(Box::new(|| Ok(Box::new(FakeMic(0)) as Box<dyn Microphone>)));
        a.run("tools.dictate", json!({})).unwrap();
        assert!(a.dictation.active());
        let ctx = egui::Context::default();
        for _ in 0..200 {
            poll(&mut a, &ctx);
            if a.dictation.run.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(a.dictation.run.is_none(), "the worker ends");
        assert!(a.status_msg.as_ref().is_some_and(|(m, _)| m.contains("config.json")), "{:?}", a.status_msg);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn translate_opens_its_dialog_and_needs_a_model() {
        let mut a = app();
        let r = a.run("review.translate", json!({})).unwrap();
        assert_eq!(r["pending"], "translate");
        let Some(crate::dialogs::Dialog::Review(d)) = a.dialog.as_mut() else { panic!("no dialog") };
        let crate::dialogs_review::ReviewDialog::Translate(f) = d.as_mut() else { panic!("not the translate dialog") };
        assert!(f.model().is_none());
        assert_eq!(f.guessed.as_deref(), Some("en"));
        assert_ne!(f.to, "en");
        let mut f = f.clone();
        let e = start_translation(&mut a, &f).unwrap_err();
        assert!(e.contains("No translation model"), "{e}");
        f.selection = true;
        assert!(a.translation.is_none());
    }

    #[test]
    fn finished_translations_go_into_a_new_document() {
        let mut a = app();
        let (tx, rx) = std::sync::mpsc::channel();
        a.translation = Some(TranslationJob {
            events: rx,
            cancel: Arc::new(AtomicBool::new(false)),
            params: json!({"to": "de", "from": "en", "scope": "document", "showIn": "new"}),
            total: 1,
            done: 0,
            loading: true,
            document: a.session.document_id(),
        });
        tx.send(Progress::Loaded).unwrap();
        tx.send(Progress::One).unwrap();
        tx.send(Progress::Finished(HashMap::from([("Good morning.".to_string(), "Guten Morgen.".to_string())]))).unwrap();
        poll_translation(&mut a, &egui::Context::default());
        assert!(a.translation.is_none());
        assert_eq!(a.session.doc.body[0].as_para().unwrap().text, "Guten Morgen.");
        a.run("edit.undo", json!({})).unwrap();
        assert_eq!(a.session.doc.body[0].as_para().unwrap().text, "Good morning.");
    }

    #[test]
    fn picking_a_model_file_keeps_its_folder() {
        assert!(picked_model_folder("speechModel", "/m/whisper-tiny/model.safetensors").ends_with("whisper-tiny"));
        // Not a complete model folder: its own folder.
        assert!(picked_model_folder("translationModels", "/m/opus-mt-en-he/vocab.json").ends_with("opus-mt-en-he"));
    }
}
