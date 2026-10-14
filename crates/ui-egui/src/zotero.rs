//! Zotero in the app: `ui.zotero.<command>` (`addEditCitation`, `addEditBibliography`, `addNote`,
//! `refresh`, `removeCodes`, `setDocPrefs`), `ui.zotero.status`, and `ui.zotero.answer
//! {"button": n}` to answer a question Zotero is asking (agents; people click the dialog).
//!
//! A command runs on a background thread that holds the socket to Zotero. Each call Zotero makes
//! is handed to the UI thread, answered against the session by [`wordcraft_zotero::Bridge`] in
//! [`poll`], and the answer goes back to the thread. Zotero's questions (`Document_displayAlert`)
//! become a dialog; the thread waits until the user answers.

use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};
use wordcraft_zotero::{Bridge, Call, Command, Host, Transaction};

use crate::WordApp;

/// A call from Zotero waiting for its answer.
struct Pending {
    call: Call,
    reply: Sender<Result<Value, String>>,
}

/// An alert Zotero is waiting on.
struct Alert {
    text: String,
    buttons: i64,
    reply: Sender<Result<Value, String>>,
}

/// The app's Zotero connection state.
#[derive(Default)]
pub struct ZoteroLink {
    /// Where Zotero listens (default `127.0.0.1:23116`).
    pub options: wordcraft_zotero::client::Options,
    bridge: Bridge,
    calls: Option<Receiver<Pending>>,
    done: Option<Receiver<Result<wordcraft_zotero::Outcome, String>>>,
    alert: Option<Alert>,
    /// The command in flight.
    pub busy: Option<Command>,
    /// Private edits, committed only after successful completion against an unchanged document.
    transaction: Option<Transaction>,
    /// How the last command ended.
    pub last: Option<Value>,
    /// Zotero's calls in the command in flight (logged when it ends).
    methods: Vec<String>,
}

/// Focus requests from the bridge.
struct AppHost<'a> {
    ctx: &'a egui::Context,
}

impl Host for AppHost<'_> {
    fn alert(&mut self, text: &str, _icon: i64, _buttons: i64) -> i64 {
        // Alerts are intercepted before the bridge (they wait for the user); this is unreachable
        // in practice and answers like Cancel.
        log::warn!("Zotero alert not shown: {text}");
        0
    }
    fn activate(&mut self) {
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
}

/// Handle `ui.zotero.*`. `None` when `id` isn't one.
pub fn command(app: &mut WordApp, id: &str, params: &Value, ctx: Option<&egui::Context>) -> Option<Result<Value, String>> {
    let name = id.strip_prefix("ui.zotero.")?;
    if name == "status" {
        return Some(Ok(status(app)));
    }
    if name == "answer" {
        let Some(button) = params.get("button").and_then(Value::as_i64) else {
            return Some(Err("give `button`: Zotero's number for it (OK/Yes 1, No/Cancel 0; with Yes/No/Cancel: Yes 2, No 1, Cancel 0)".into()));
        };
        return Some(answer(app, button));
    }
    let Some(cmd) = Command::from_name(name) else {
        let names: Vec<&str> = Command::ALL.iter().map(|c| c.wire_name()).collect();
        return Some(Err(format!("unknown Zotero command `{name}`; use one of {}", names.join(", "))));
    };
    Some(start(app, cmd, ctx.cloned()))
}

fn status(app: &WordApp) -> Value {
    let z = &app.zotero;
    json!({"busy": z.busy.map(Command::wire_name), "waitingForAnswer": z.alert.as_ref().map(|a| a.text.clone()), "last": z.last})
}

#[cfg(target_arch = "wasm32")]
fn start(_app: &mut WordApp, _cmd: Command, _ctx: Option<egui::Context>) -> Result<Value, String> {
    Err("Zotero works with the WordCraft desktop app".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn start(app: &mut WordApp, cmd: Command, ctx: Option<egui::Context>) -> Result<Value, String> {
    use std::sync::mpsc::channel;
    if let Some(b) = app.zotero.busy {
        return Err(format!("Zotero is still working on {}; finish in Zotero's window first", b.wire_name()));
    }
    let transaction = Transaction::new(&app.session, &app.zotero.bridge);
    let (call_tx, call_rx) = channel::<Pending>();
    let (done_tx, done_rx) = channel();
    let wake = ctx.clone();
    let opts = app.zotero.options.clone();
    let spawned = std::thread::Builder::new().name("zotero".into()).spawn(move || {
        let r = wordcraft_zotero::client::run_command(&opts, cmd, &mut |call| {
            let (tx, rx) = channel();
            if call_tx.send(Pending { call: call.clone(), reply: tx }).is_err() {
                return Err("WordCraft closed the document".into());
            }
            if let Some(c) = &wake {
                c.request_repaint();
            }
            rx.recv().unwrap_or_else(|_| Err("WordCraft closed the document".into()))
        });
        let _ = done_tx.send(r.map_err(|e| e.to_string()));
        if let Some(c) = &wake {
            c.request_repaint();
        }
    });
    if let Err(e) = spawned {
        return Err(format!("could not start the Zotero connection: {e}"));
    }
    let z = &mut app.zotero;
    z.calls = Some(call_rx);
    z.done = Some(done_rx);
    z.busy = Some(cmd);
    z.transaction = Some(transaction);
    log::info!("Zotero: {} started", cmd.wire_name());
    app.status(tl!("Waiting for Zotero…"));
    Ok(json!({"started": cmd.wire_name()}))
}

/// Answer the question Zotero is waiting on with `button` (Zotero's numbering).
pub fn answer(app: &mut WordApp, button: i64) -> Result<Value, String> {
    let Some(a) = app.zotero.alert.take() else { return Err("Zotero isn't asking anything".into()) };
    let max = match a.buttons {
        3 => 2,
        _ => 1,
    };
    if !(0..=max).contains(&button) {
        let text = a.text.clone();
        app.zotero.alert = Some(a);
        return Err(format!("button must be 0–{max} for this question: {text}"));
    }
    let _ = a.reply.send(Ok(json!(button)));
    if let Some(c) = &app.ctx {
        c.request_repaint();
    }
    Ok(json!({"answered": button}))
}

/// Answer Zotero's calls (each frame).
pub fn poll(app: &mut WordApp, ctx: &egui::Context) {
    if app.zotero.busy.is_none() {
        return;
    }
    // One alert at a time: Zotero sends nothing else while it waits for the answer.
    while app.zotero.alert.is_none() {
        let Some(p) = app.zotero.calls.as_ref().and_then(|rx| rx.try_recv().ok()) else { break };
        app.zotero.methods.push(p.call.method.clone());
        let current = app
            .zotero
            .transaction
            .as_ref()
            .ok_or_else(|| "Zotero transaction is missing".to_string())
            .and_then(|t| t.check_current(&app.session).map_err(|e| e.to_string()));
        if let Err(e) = current {
            let _ = p.reply.send(Err(e));
            continue;
        }
        if p.call.method == "Document_displayAlert" {
            let text = p.call.args.get(1).and_then(Value::as_str).unwrap_or("").to_string();
            let buttons = p.call.args.get(3).and_then(Value::as_i64).unwrap_or(0);
            app.zotero.alert = Some(Alert { text, buttons, reply: p.reply });
            continue;
        }
        let mut host = AppHost { ctx };
        let r = app.zotero.transaction.as_mut().ok_or_else(|| "Zotero transaction is missing".to_string()).and_then(|t| t.handle(&mut host, &p.call));
        if r.is_ok() && app.zotero.transaction.as_mut().is_some_and(|t| t.preview_selection(&mut app.session, &p.call)) {
            app.canvas.scroll_to_caret = true;
            app.canvas.caret_visible_since = crate::now_ms();
        }
        let _ = p.reply.send(r);
    }
    let done = app.zotero.done.as_ref().and_then(|rx| rx.try_recv().ok());
    if let Some(r) = done {
        let cmd = app.zotero.busy.take();
        app.zotero.calls = None;
        app.zotero.done = None;
        app.zotero.alert = None;
        let transaction = app.zotero.transaction.take();
        let r = match (r, transaction) {
            (Ok(out), Some(t)) => t
                .commit(&mut app.session, &mut app.zotero.bridge, &out)
                .map(|()| {
                    app.canvas.scroll_to_caret = true;
                    app.canvas.caret_visible_since = crate::now_ms();
                    out
                })
                .map_err(|e| e.to_string()),
            (Err(e), t) => {
                if let Some(t) = t {
                    t.discard(&mut app.session);
                }
                Err(e)
            }
            (Ok(_), None) => Err("Zotero transaction is missing".into()),
        };
        let name = cmd.map(Command::wire_name).unwrap_or("");
        let methods = std::mem::take(&mut app.zotero.methods).join(", ");
        match &r {
            Ok(o) => log::info!(
                "Zotero: {name} {} after {} calls ({methods}){}",
                if o.completed { "completed" } else { "ended without completing" },
                o.calls,
                if o.errors.is_empty() { String::new() } else { format!("; errors: {}", o.errors.join(" | ")) }
            ),
            Err(e) => log::warn!("Zotero: {name} failed: {e}"),
        }
        match r {
            Ok(o) => {
                app.zotero.last = Some(json!({"command": name, "completed": o.completed, "calls": o.calls, "errors": o.errors}));
                app.status(if o.completed { tl!("Zotero: done") } else { tl!("Zotero: cancelled") });
            }
            Err(e) => {
                app.zotero.last = Some(json!({"command": name, "error": e}));
                app.status(e);
            }
        }
        app.canvas.want_focus = true;
    }
}

/// Zotero's question, as a dialog.
pub fn show_alert(app: &mut WordApp, ctx: &egui::Context) {
    let Some(a) = app.zotero.alert.as_ref() else { return };
    // Buttons and the value Zotero expects for each.
    let buttons: &[(&str, i64)] = match a.buttons {
        1 => &[("OK", 1), ("Cancel", 0)],
        2 => &[("Yes", 1), ("No", 0)],
        3 => &[("Yes", 2), ("No", 1), ("Cancel", 0)],
        _ => &[("OK", 1)],
    };
    let mut picked = None;
    egui::Window::new("Zotero")
        .id(egui::Id::new("zotero-alert"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.label(a.text.trim_end());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (label, v) in buttons {
                    if ui.button(tl!(label)).clicked() {
                        picked = Some(*v);
                    }
                }
            });
        });
    if let Some(v) = picked {
        let _ = answer(app, v);
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use serde_json::{Value, json};
    use wordcraft_doc::{Document, StoryRef};
    use wordcraft_engine::Session;
    use wordcraft_zotero::wire;

    use crate::{Services, WordApp};

    /// A fake Zotero making `calls`; returns the replies.
    fn fake_zotero(calls: Vec<Value>) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let _command = wire::read_frame(&mut s).unwrap().unwrap();
            let mut replies = Vec::new();
            for (i, c) in calls.into_iter().enumerate() {
                wire::write_frame(&mut s, 1 + i as u32, &serde_json::to_vec(&c).unwrap()).unwrap();
                replies.push(String::from_utf8(wire::read_frame(&mut s).unwrap().unwrap().payload).unwrap());
            }
            replies
        });
        (addr, h)
    }

    /// Run frames until `done` or a few seconds pass.
    fn pump(app: &mut WordApp, ctx: &egui::Context, done: impl Fn(&WordApp) -> bool) {
        for _ in 0..500 {
            super::poll(app, ctx);
            if done(app) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out");
    }

    #[test]
    fn a_zotero_session_runs_through_the_app_with_a_question() {
        let (addr, server) = fake_zotero(vec![
            json!(["Application_getActiveDocument", [3]]),
            json!(["Document_insertField", [1, "ReferenceMark", 0]]),
            json!(["Field_setCode", [1, 1, "ITEM CSL_CITATION {}"]]),
            json!(["Document_displayAlert", [1, "Keep going?", 1, 2]]),
            json!(["Field_setText", [1, 1, "{\\rtf (Doe, {\\i 2020})}", true]]),
            json!(["Document_complete", [1]]),
        ]);
        let mut app = WordApp::new(Session::new(Document::from_text("Cite: ")), Services::default());
        app.session.sel = wordcraft_engine::Selection::caret(wordcraft_doc::Pos::body(0, 6));
        app.zotero.options.addr = addr;
        let ctx = egui::Context::default();
        app.ctx = Some(ctx.clone());
        assert_eq!(app.run("ui.zotero.addEditCitation", json!({})).unwrap(), json!({"started": "addEditCitation"}));
        assert!(app.run("ui.zotero.refresh", json!({})).is_err(), "one session at a time");
        pump(&mut app, &ctx, |a| a.zotero.alert.is_some());
        assert_eq!(app.session.doc.plain_text(StoryRef::Body), "Cite: ", "partial fields never reach the live document or autosave");
        assert!(!app.session.can_undo());
        assert!(!app.session.dirty);
        let st = app.run("ui.zotero.status", json!({})).unwrap();
        assert_eq!(st["waitingForAnswer"], "Keep going?");
        assert!(app.run("ui.zotero.answer", json!({"button": 7})).is_err());
        assert_eq!(app.run("ui.zotero.answer", json!({"button": 1})).unwrap(), json!({"answered": 1}));
        pump(&mut app, &ctx, |a| a.zotero.busy.is_none());
        let replies = server.join().unwrap();
        assert_eq!(replies[3], "1");
        assert_eq!(app.session.doc.plain_text(StoryRef::Body), "Cite: (Doe, 2020)");
        assert_eq!(app.run("ui.zotero.status", json!({})).unwrap()["last"]["completed"], true);
        assert!(app.run("ui.zotero.answer", json!({"button": 1})).is_err());
        assert!(app.run("ui.zotero.nope", json!({})).is_err());
        // One undo step.
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(app.session.doc.plain_text(StoryRef::Body), "Cite: ");
    }

    #[test]
    fn a_pending_citation_does_not_insert_into_a_replacement_document() {
        let (addr, server) = fake_zotero(vec![
            json!(["Application_getActiveDocument", [3]]),
            json!(["Document_insertField", [1, "ReferenceMark", 0]]),
            json!(["Field_setCode", [1, 1, "ITEM CSL_CITATION {}"]]),
            json!(["Field_setText", [1, 1, "(Sample, 2020)", false]]),
            json!(["Document_complete", [1]]),
        ]);
        let mut app = WordApp::new(Session::new(Document::new()), Services::default());
        app.zotero.options.addr = addr;
        let ctx = egui::Context::default();
        app.ctx = Some(ctx.clone());
        app.run("ui.zotero.addEditCitation", json!({})).unwrap();
        app.run("file.new", json!({"template": "blank"})).unwrap();
        pump(&mut app, &ctx, |a| a.zotero.busy.is_none());
        server.join().unwrap();
        assert_eq!(app.session.doc.plain_text(StoryRef::Body), "");
        assert!(!app.session.can_undo(), "the old command left no undo step in the new document");
    }

    #[test]
    fn a_cancelled_or_failed_command_discards_its_edits() {
        for fails in [false, true] {
            let mut calls = vec![json!(["Application_getActiveDocument", [3]]), json!(["Document_insertText", [1, "partial"]])];
            if fails {
                calls.push(json!(["Not_a_method", []]));
                calls.push(json!(["Document_complete", [1]]));
            }
            let (addr, server) = fake_zotero(calls);
            let mut app = WordApp::new(Session::new(Document::from_text("original")), Services::default());
            let before = serde_json::to_value(&app.session.doc).unwrap();
            let sel = app.session.sel.clone();
            app.zotero.options.addr = addr;
            let ctx = egui::Context::default();
            app.run("ui.zotero.refresh", json!({})).unwrap();
            pump(&mut app, &ctx, |a| a.zotero.busy.is_none());
            server.join().unwrap();
            assert_eq!(serde_json::to_value(&app.session.doc).unwrap(), before);
            assert_eq!(app.session.sel, sel);
            assert!(!app.session.dirty);
            assert!(!app.session.can_undo());
            assert!(app.zotero.transaction.is_none());
            assert!(app.zotero.last.as_ref().unwrap()["error"].is_string());
        }
    }

    #[test]
    fn editing_or_replacing_the_document_during_a_dialog_keeps_the_users_work() {
        for replace in [false, true] {
            let (addr, server) = fake_zotero(vec![
                json!(["Application_getActiveDocument", [3]]),
                json!(["Document_insertText", [1, "partial"]]),
                json!(["Document_displayAlert", [1, "Continue?", 1, 1]]),
                json!(["Document_complete", [1]]),
            ]);
            let mut app = WordApp::new(Session::new(Document::from_text("original")), Services::default());
            app.zotero.options.addr = addr;
            let ctx = egui::Context::default();
            app.run("ui.zotero.refresh", json!({})).unwrap();
            pump(&mut app, &ctx, |a| a.zotero.alert.is_some());
            if replace {
                app.run("file.new", json!({"template": "blank"})).unwrap();
            }
            app.run("text.insert", json!({"text": "my work"})).unwrap();
            let before = serde_json::to_value(&app.session.doc).unwrap();
            let sel = app.session.sel.clone();
            let undo = app.session.undo_labels();
            app.run("ui.zotero.answer", json!({"button": 1})).unwrap();
            pump(&mut app, &ctx, |a| a.zotero.busy.is_none());
            server.join().unwrap();
            assert_eq!(serde_json::to_value(&app.session.doc).unwrap(), before);
            assert_eq!(app.session.sel, sel);
            assert_eq!(app.session.undo_labels(), undo);
            assert!(app.zotero.last.as_ref().unwrap()["error"].as_str().unwrap().contains("document changed"));
        }
    }

    #[test]
    fn existing_citation_previews_use_original_positions_and_restore_only_their_own_selection() {
        for ending in ["cancel", "complete", "user-moved"] {
            let mut session = Session::new(Document::from_text("original"));
            let mut bridge = wordcraft_zotero::Bridge::new();
            let mut host = wordcraft_zotero::Headless::default();
            for call in [
                wordcraft_zotero::Call::new("Application_getActiveDocument", vec![json!(3)]),
                wordcraft_zotero::Call::new("Document_insertField", vec![json!(1), json!("ReferenceMark"), json!(0)]),
                wordcraft_zotero::Call::new("Field_setCode", vec![json!(1), json!(1), json!("ITEM CSL_CITATION {}")]),
                wordcraft_zotero::Call::new("Document_complete", vec![json!(1)]),
            ] {
                bridge.handle(&mut session, &mut host, &call).unwrap();
            }
            let original = serde_json::to_value(&session.doc).unwrap();
            let start_sel = session.sel.clone();
            let undo = session.undo_labels();
            let field = wordcraft_zotero::fields::list(&session.doc).remove(0);
            let expected_preview = wordcraft_engine::Selection { anchor: field.start.clone(), focus: field.after() };
            let mut calls = vec![
                json!(["Application_getActiveDocument", [3]]),
                json!(["Field_setText", [1, 1, "a longer citation", false]]),
                json!(["Field_select", [1, 1]]),
                json!(["Document_displayAlert", [1, "Keep this citation?", 1, 1]]),
            ];
            if ending == "complete" {
                calls.push(json!(["Document_complete", [1]]));
            }
            let (addr, server) = fake_zotero(calls);
            let mut app = WordApp::new(session, Services::default());
            app.zotero.options.addr = addr;
            let ctx = egui::Context::default();
            app.run("ui.zotero.refresh", json!({})).unwrap();
            pump(&mut app, &ctx, |a| a.zotero.alert.is_some());
            assert_eq!(app.session.sel, expected_preview);
            assert_eq!(serde_json::to_value(&app.session.doc).unwrap(), original);
            if ending == "user-moved" {
                app.session.sel = wordcraft_engine::Selection::caret(app.session.doc.end_of(StoryRef::Body));
            }
            let expected = if ending == "user-moved" { app.session.sel.clone() } else { start_sel };
            app.run("ui.zotero.answer", json!({"button": 1})).unwrap();
            pump(&mut app, &ctx, |a| a.zotero.busy.is_none());
            server.join().unwrap();
            assert_eq!(app.session.sel, expected);
            if ending != "complete" {
                assert_eq!(serde_json::to_value(&app.session.doc).unwrap(), original);
                assert_eq!(app.session.undo_labels(), undo);
            } else {
                assert!(app.session.doc.plain_text(StoryRef::Body).contains("a longer citation"));
            }
        }
    }

    #[test]
    fn no_zotero_is_a_clear_error() {
        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let mut app = WordApp::new(Session::new(Document::new()), Services::default());
        app.zotero.options.addr = free;
        let ctx = egui::Context::default();
        app.run("ui.zotero.refresh", json!({})).unwrap();
        pump(&mut app, &ctx, |a| a.zotero.busy.is_none());
        let last = app.run("ui.zotero.status", json!({})).unwrap()["last"].clone();
        assert!(last["error"].as_str().unwrap().contains("Zotero isn't running"), "{last}");
    }
}
