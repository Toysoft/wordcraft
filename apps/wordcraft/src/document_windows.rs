//! One document per desktop process. Launch the executable directly (also on macOS), so
//! LaunchServices cannot redirect a new document back to the existing process.

use std::{
    cell::RefCell,
    process::{Child, Command},
    rc::Rc,
};
use wordcraft_ui_egui::{DocumentWindow, DocumentWindowHook};

/// Keep child handles until they exit, reaping them without blocking the UI. Closing this
/// window drops the handles, not the children: their documents and save prompts stay alive.
#[derive(Default)]
pub struct Launcher(Rc<RefCell<Vec<Child>>>);

impl Launcher {
    pub fn poll(&self) {
        if let Ok(mut children) = self.0.try_borrow_mut() {
            children.retain_mut(|child| match child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) => true,
                Err(e) => {
                    log::warn!("document window: couldn't check child: {e}");
                    false
                }
            });
        }
    }

    pub fn hook(&self) -> DocumentWindowHook {
        let children = self.0.clone();
        Box::new(move |request| {
            let mut children = children.try_borrow_mut().map_err(|e| format!("Couldn't open another window: {e}"))?;
            let executable = std::env::current_exe().map_err(|e| format!("Couldn't find WordCraft: {e}"))?;
            let child = command(executable, request).spawn().map_err(|e| format!("Couldn't open another window: {e}"))?;
            children.push(child);
            Ok(())
        })
    }
}

fn command(executable: std::path::PathBuf, request: DocumentWindow) -> Command {
    let mut command = Command::new(executable);
    // Control is opt-in per process. Inheriting the parent's port would collide with its server.
    command.env_remove("WORDCRAFT_CONTROL_PORT");
    match request {
        DocumentWindow::New { template } => {
            command.args(["--template", &template]);
        }
        DocumentWindow::Open { path } => {
            // Paths such as `--sample` must remain file names, not become options.
            command.args(["--", &path]);
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn child_arguments_preserve_paths_and_do_not_inherit_control_port() {
        let path = "--résumé with spaces.docx";
        let c = command("wordcraft".into(), DocumentWindow::Open { path: path.into() });
        assert_eq!(c.get_args().collect::<Vec<_>>(), [OsStr::new("--"), OsStr::new(path)]);
        assert!(c.get_envs().any(|(name, value)| name == "WORDCRAFT_CONTROL_PORT" && value.is_none()));
        assert_eq!(c.get_program(), "wordcraft");
    }

    #[test]
    fn new_document_carries_its_template() {
        let c = command("wordcraft".into(), DocumentWindow::New { template: "report".into() });
        assert_eq!(c.get_args().collect::<Vec<_>>(), [OsStr::new("--template"), OsStr::new("report")]);
    }

    #[test]
    fn reaps_finished_children() {
        let launcher = Launcher::default();
        // This test never opens the graphical app.
        let mut child = Command::new(std::env::current_exe().unwrap()).arg("--list").stdout(std::process::Stdio::null()).spawn().unwrap();
        child.wait().unwrap();
        launcher.0.borrow_mut().push(child);
        launcher.poll();
        assert!(launcher.0.borrow().is_empty());
    }
}
