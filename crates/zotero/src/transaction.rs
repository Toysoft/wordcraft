//! Keep an integration command private until the peer completes it without errors.
use serde_json::Value;
use wordcraft_engine::{Selection, Session};

use crate::{Bridge, Call, Host, Outcome, ZoteroError};

/// One Zotero command, including all of its callbacks. Dropping it discards its edits and
/// bridge bookkeeping. The live session (and its undo/redo stacks) is untouched until commit.
/// This also keeps autosave and edits made while Zotero's dialog is open away from partial work.
pub struct Transaction {
    session: Session,
    bridge: Bridge,
    document: u64,
    revision: u64,
    selection: Selection,
    original_bridge: Bridge,
    preview: Option<Selection>,
    failed: Option<String>,
}

impl Transaction {
    pub fn new(session: &Session, bridge: &Bridge) -> Self {
        // Bridge dispatch only edits the document/selection, or runs the two note insertion
        // commands. Copy their editing context, never file/clipboard/UI side effects.
        let mut staged = Session::new(session.doc.clone());
        staged.sel = session.sel.clone();
        staged.pending = session.pending.clone();
        staged.column = session.column.clone();
        staged.author = session.author.clone();
        staged.initials = session.initials.clone();
        staged.prefs = session.prefs.clone();
        staged.registry = session.registry.clone();
        let mut bridge = bridge.clone();
        bridge.begin(&staged);
        bridge.prepare_fields(&staged);
        Self {
            session: staged,
            original_bridge: bridge.clone(),
            bridge,
            document: session.document_id(),
            revision: session.rev(),
            selection: session.sel.clone(),
            preview: None,
            failed: None,
        }
    }

    /// Refuse stale work, including an edit followed by Undo or a replacement document.
    pub fn check_current(&self, session: &Session) -> Result<(), ZoteroError> {
        if session.document_id() != self.document || session.rev() != self.revision {
            return Err(ZoteroError::Failed("the document changed while Zotero was working; try again".into()));
        }
        Ok(())
    }

    /// Answer against the private document. A callback error makes the whole command fail,
    /// even if the peer subsequently sends Document_complete.
    pub fn handle(&mut self, host: &mut dyn Host, call: &Call) -> Result<Value, String> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        let result = self.bridge.handle(&mut self.session, host, call);
        if let Err(e) = &result {
            self.failed = Some(e.clone());
        }
        result
    }

    /// Optionally show an existing citation while Zotero asks the user about it. Positions
    /// come from the original document, even if staged edits have moved or deleted fields.
    /// Newly inserted fields remain private until commit and cannot be previewed this way.
    pub fn preview_selection(&mut self, session: &mut Session, call: &Call) -> bool {
        if call.method != "Field_select" || self.failed.is_some() || self.check_current(session).is_err() {
            return false;
        }
        let Ok(sel) = self.original_bridge.field_selection(session, call) else { return false };
        session.sel = sel.clone();
        self.preview = Some(sel);
        true
    }

    fn restore_preview(&self, session: &mut Session) {
        if self.check_current(session).is_ok() && self.preview.as_ref() == Some(&session.sel) {
            session.sel = self.selection.clone();
        }
    }

    /// Discard an interrupted command, restoring only a selection preview still owned by it.
    pub fn discard(self, session: &mut Session) {
        self.restore_preview(session);
    }

    /// Publish a fully successful command as one undo step. On every error, the live document,
    /// selection, dirty flag, history and bridge remain as they were before this call.
    pub fn commit(self, session: &mut Session, bridge: &mut Bridge, outcome: &Outcome) -> Result<(), ZoteroError> {
        let valid = outcome.ensure_success().and_then(|()| {
            if let Some(e) = &self.failed {
                return Err(ZoteroError::Failed(e.clone()));
            }
            if !self.bridge.completed {
                return Err(ZoteroError::Incomplete);
            }
            self.check_current(session)
        });
        if let Err(e) = valid {
            self.restore_preview(session);
            return Err(e);
        }
        let same_selection = session.sel == self.selection || self.preview.as_ref() == Some(&session.sel);
        if self.session.can_undo() {
            session.checkpoint("Zotero");
            session.doc = self.session.doc;
            session.touch();
        }
        // A user may move the caret while a dialog is open; do not move it back on completion.
        if same_selection {
            session.sel = self.session.sel;
            session.pending = self.session.pending;
            session.column = self.session.column;
        }
        session.clamp_selection();
        *bridge = self.bridge;
        Ok(())
    }
}
