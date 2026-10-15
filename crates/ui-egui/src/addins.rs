//! Add-ins: optional modules a host app ships (File › Options › Add-ins turns each on or off).
//!
//! The host hands the app its add-ins ([`WordApp::add_addin`]); the interface knows nothing about
//! what any of them does. Each one is a module id in the editing preferences (`modules.<id>`, so
//! `file.options {"modules": {"<id>": true}}` turns it on from a script too), installs itself into
//! the session when its state changes, and may bring its own File › Options page, listed under its
//! name while it is on.

use egui::{RichText, Ui};
use serde_json::{Value, json};
use wordcraft_engine::Session;

use crate::WordApp;
use crate::file_dialogs::FileDialogRequest;

/// The options pane id of an add-in's page.
pub fn pane_id(id: &str) -> String {
    format!("addin:{id}")
}

/// An optional module the host ships.
pub trait AddIn {
    /// Its module id in the editing preferences (`modules.<id>`).
    fn id(&self) -> &'static str;
    /// Its name in File › Options › Add-ins and on its options page.
    fn name(&self) -> &str;
    /// One line saying what it does.
    fn description(&self) -> &str;
    /// Apply the session's setting for this module: install its commands (and anything else it
    /// adds) when it is on, take them away when it is off. Called once the app has the user's
    /// preferences, and again whenever the setting changes.
    fn install(&mut self, session: &mut Session);
    /// Whether it has a File › Options page.
    fn has_options_page(&self) -> bool {
        false
    }
    /// Draw its File › Options page (only while it is on).
    fn options_page(&mut self, _app: &mut WordApp, _ui: &mut Ui) {}
    /// The person is leaving its options page, or turning it off: save what is pending. `false`
    /// keeps the page open (it shows why).
    fn leave_page(&mut self, _app: &mut WordApp) -> bool {
        true
    }
    /// A file it asked for with [`WordApp::addin_ask_file`] came back (`None`: cancelled).
    fn file_picked(&mut self, _app: &mut WordApp, _tag: &str, _picked: Option<String>) -> Result<Value, String> {
        Ok(json!({"cancelled": true}))
    }
    /// For tests and hosts that need the concrete add-in.
    fn as_any(&self) -> &dyn std::any::Any;
    /// For tests and hosts that need the concrete add-in.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

impl WordApp {
    /// Give the app an add-in and apply the user's setting for it.
    pub fn add_addin(&mut self, mut addin: Box<dyn AddIn>) {
        addin.install(&mut self.session);
        self.addins.push(addin);
    }

    /// Run `f` with the add-in `id` taken out of the app, so it can use the app; `None` when the
    /// app has no such add-in.
    pub fn with_addin<R>(&mut self, id: &str, f: impl FnOnce(&mut dyn AddIn, &mut WordApp) -> R) -> Option<R> {
        let i = self.addins.iter().position(|a| a.id() == id)?;
        let mut addin = self.addins.remove(i);
        let r = f(addin.as_mut(), self);
        self.addins.insert(i.min(self.addins.len()), addin);
        Some(r)
    }

    /// The add-in `id` as its own type (tests, hosts).
    pub fn addin<T: 'static>(&self, id: &str) -> Option<&T> {
        self.addins.iter().find(|a| a.id() == id).and_then(|a| a.as_any().downcast_ref::<T>())
    }

    /// The add-in `id` as its own type (tests, hosts).
    pub fn addin_mut<T: 'static>(&mut self, id: &str) -> Option<&mut T> {
        self.addins.iter_mut().find(|a| a.id() == id).and_then(|a| a.as_any_mut().downcast_mut::<T>())
    }

    /// Ask for a file for the add-in `id`; its [`AddIn::file_picked`] gets the answer with `tag`.
    pub fn addin_ask_file(&mut self, id: &str, req: FileDialogRequest, tag: &str) {
        let _ = self.ask_file(req, crate::file_dialogs::AfterPick::AddIn { id: id.to_string(), tag: tag.to_string() });
    }

    /// The add-in whose options page is showing, if any.
    pub(crate) fn addin_on_page(&self) -> Option<String> {
        if !(self.ui.backstage && self.ui.backstage_page == "options") {
            return None;
        }
        let id = self.ui.options_pane.strip_prefix("addin:")?;
        self.session.module_enabled(id).then(|| id.to_string())
    }

    /// Leaving the add-in options page that is showing (another pane, another page, closing
    /// File): let it save. `false`: it stays.
    pub(crate) fn leave_addin_page(&mut self) -> bool {
        match self.addin_on_page() {
            Some(id) => self.with_addin(&id, |a, app| a.leave_page(app)).unwrap_or(true),
            None => true,
        }
    }

    /// Before `file.options` turns an add-in off, let it save what its page has pending; an error
    /// keeps it on.
    pub(crate) fn guard_addin_off(&mut self, id: &str, params: &Value) -> Result<(), String> {
        if id != "file.options" {
            return Ok(());
        }
        let Some(modules) = params.get("modules").and_then(Value::as_object) else { return Ok(()) };
        let off: Vec<String> = modules.iter().filter(|(_, v)| v.as_bool() == Some(false)).map(|(k, _)| k.clone()).collect();
        for m in off {
            let name = self.addins.iter().find(|a| a.id() == m).map_or_else(|| m.clone(), |a| a.name().to_string());
            if self.session.module_enabled(&m) && self.with_addin(&m, |a, app| a.leave_page(app)) == Some(false) {
                return Err(format!("{name}: pending changes on its options page could not be saved"));
            }
        }
        Ok(())
    }

    /// After `file.options` changed modules, apply each add-in's setting; a page of one turned
    /// off is left for the Add-ins page.
    pub(crate) fn sync_addins(&mut self) {
        let mut addins = std::mem::take(&mut self.addins);
        for a in &mut addins {
            a.install(&mut self.session);
        }
        self.addins = addins;
        if let Some(id) = self.ui.options_pane.strip_prefix("addin:")
            && !self.session.module_enabled(id)
        {
            self.ui.options_pane = "addins".into();
        }
    }

    /// The options panes the add-ins bring, (pane id, name), for those that are on.
    pub(crate) fn addin_panes(&self) -> Vec<(String, String)> {
        self.addins
            .iter()
            .filter(|a| a.has_options_page() && self.session.module_enabled(a.id()))
            .map(|a| (pane_id(a.id()), a.name().to_string()))
            .collect()
    }
}

/// File › Options › Add-ins: each add-in the app has, with its switch.
pub fn page(app: &mut WordApp, ui: &mut Ui) {
    ui.add_space(6.0);
    ui.label(RichText::new(tl!("Add-ins")).font(crate::theme::semibold(15.0)));
    ui.add_space(2.0);
    if app.addins.is_empty() {
        ui.label(tl!("No add-ins are installed."));
        return;
    }
    ui.label(tl!("Each add-in stays off until you turn it on."));
    ui.add_space(6.0);
    let list: Vec<(String, String, String)> =
        app.addins.iter().map(|a| (a.id().to_string(), a.name().to_string(), a.description().to_string())).collect();
    for (id, name, description) in list {
        let on = app.session.module_enabled(&id);
        let mut checked = on;
        // Why the last change didn't go through (an add-in that couldn't save stays on), shown
        // under its switch until the next change.
        let error_id = egui::Id::new(("addin_error", id.as_str()));
        if ui.checkbox(&mut checked, name.as_str()).changed() {
            match app.run("file.options", json!({"modules": {id.clone(): checked}})) {
                Err(e) => {
                    ui.data_mut(|d| d.insert_temp(error_id, e));
                }
                Ok(_) => {
                    ui.data_mut(|d| {
                        d.remove::<String>(error_id);
                    });
                    if checked && app.addins.iter().any(|a| a.id() == id && a.has_options_page()) {
                        app.ui.options_pane = pane_id(&id);
                    }
                }
            }
        }
        if let Some(e) = ui.data(|d| d.get_temp::<String>(error_id)) {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        ui.label(RichText::new(description).small().weak());
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wordcraft_doc::Document;

    use super::*;
    use crate::Services;

    /// An add-in that counts what the app asks of it.
    #[derive(Default)]
    struct Probe {
        installs: usize,
        pages: usize,
        refuse_leave: bool,
        picked: Option<(String, Option<String>)>,
    }

    impl AddIn for Probe {
        fn id(&self) -> &'static str {
            "probe"
        }
        fn name(&self) -> &str {
            "Probe"
        }
        fn description(&self) -> &str {
            "A test add-in."
        }
        fn install(&mut self, _session: &mut Session) {
            self.installs += 1;
        }
        fn has_options_page(&self) -> bool {
            true
        }
        fn options_page(&mut self, _app: &mut WordApp, _ui: &mut Ui) {
            self.pages += 1;
        }
        fn leave_page(&mut self, _app: &mut WordApp) -> bool {
            !self.refuse_leave
        }
        fn file_picked(&mut self, _app: &mut WordApp, tag: &str, picked: Option<String>) -> Result<Value, String> {
            self.picked = Some((tag.to_string(), picked));
            Ok(json!({"done": true}))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    fn app() -> WordApp {
        let mut a = WordApp::new(Session::new(Document::new()), Services::default());
        a.add_addin(Box::new(Probe::default()));
        a
    }

    fn probe(a: &mut WordApp) -> &mut Probe {
        a.addin_mut::<Probe>("probe").unwrap()
    }

    #[test]
    fn the_switch_is_a_module_setting_and_the_page_comes_with_it() {
        let mut a = app();
        assert_eq!(probe(&mut a).installs, 1, "added with the user's setting applied");
        assert!(a.addin_panes().is_empty(), "no page while it is off");
        a.run("file.options", json!({"modules": {"probe": true}})).unwrap();
        assert!(a.session.module_enabled("probe"));
        assert_eq!(probe(&mut a).installs, 2, "turning it on applies the setting again");
        assert_eq!(a.addin_panes(), [("addin:probe".to_string(), "Probe".to_string())]);
        a.run("file.options", json!({"pane": "addin:probe"})).unwrap();
        assert_eq!(a.ui.options_pane, "addin:probe");
        a.run("file.options", json!({"modules": {"probe": false}})).unwrap();
        assert!(!a.session.module_enabled("probe"));
        assert_eq!(a.ui.options_pane, "addins", "its page goes with it");
        assert!(a.run("file.options", json!({"modules": {"probe": "yes"}})).is_err());
    }

    #[test]
    fn its_page_can_hold_the_person_until_it_has_saved() {
        let mut a = app();
        a.run("file.options", json!({"modules": {"probe": true}})).unwrap();
        a.run("file.options", json!({"pane": "addin:probe"})).unwrap();
        probe(&mut a).refuse_leave = true;
        a.run("ui.backstage", json!({"pane": "general"})).unwrap();
        assert_eq!(a.ui.options_pane, "addin:probe", "another pane waits");
        a.run("ui.backstage", json!({"value": false})).unwrap();
        assert!(a.ui.backstage, "closing File waits");
        assert!(a.run("file.options", json!({"modules": {"probe": false}})).is_err(), "turning it off waits");
        assert!(a.session.module_enabled("probe"));
        probe(&mut a).refuse_leave = false;
        a.run("ui.backstage", json!({"value": false})).unwrap();
        assert!(!a.ui.backstage);
    }

    #[test]
    fn a_file_it_asks_for_comes_back_to_it() {
        let mut a =
            WordApp::new(Session::new(Document::new()), Services { pick_open: Some(Box::new(|_| Some("/tmp/x.json".into()))), ..Default::default() });
        a.add_addin(Box::new(Probe::default()));
        a.addin_ask_file("probe", FileDialogRequest::Open { purpose: "json".into() }, "import");
        assert_eq!(probe(&mut a).picked, Some(("import".to_string(), Some("/tmp/x.json".to_string()))));
    }

    #[test]
    fn the_add_ins_page_lists_each_add_in_with_its_switch() {
        let mut a = app();
        a.run("file.options", json!({"pane": "addins"})).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_ui_state(
            |ui, app: &mut WordApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(4);
        use egui_kittest::kittest::Queryable;
        h.get_by_label("Probe").click();
        h.run_steps(4);
        assert!(h.state().session.module_enabled("probe"));
        assert_eq!(h.state().ui.options_pane, "addin:probe", "turning it on opens its page");
        assert!(h.state_mut().addin_mut::<Probe>("probe").unwrap().pages > 0, "and draws it");
    }
}
