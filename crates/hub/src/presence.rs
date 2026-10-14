//! The presence projection (CraftHub spec §6.1, §18.4 row 2): what WordCraft's live UI state
//! looks like on the wire, and what a peer's state renders as here. Presence is ephemeral — it
//! is never stored, never journalled, never saved (§2.7's rule, kept verbatim in v3).
//!
//! WordCraft's row in the per-app projection table is the word-processor shape: the caret in a
//! paragraph (the `editing_text` triple) is the live signal — a peer writing in paragraph forty
//! shows as a badge in that paragraph's margin, not a floating dot in a canvas. The page number
//! the peer is on rides `page`, the selection rides `selection` (paragraph ids), and the
//! viewport rectangle is offered to the follow mode. WordCraft has no timeline, no tracks and
//! no generation jobs — those fields belong to the siblings and stay `None` here.

use craft_collab::adapter::UiState;

/// Everything the WordCraft UI needs to render one peer: the projection is a two-way street —
/// the same fields go out and come back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeerView {
    /// The peer's cursor in page coordinates, on the page they're on.
    pub cursor: Option<(f64, f64)>,
    /// The page the peer is on (a peer on another page renders only in the page bar).
    pub page: Option<u64>,
    /// The peer's selection (paragraph ids — outline highlights, never edit locks; §6.7 does locks).
    pub selection: Vec<craft_collab::ids::NodeId>,
    /// The peer's viewport, offered to the follow mode.
    pub viewport: Option<(f64, f64, f64, f64)>,
    /// The peer is writing in this paragraph (caret and selection in bytes within it).
    pub editing_text: Option<(craft_collab::ids::NodeId, usize, Option<usize>)>,
}

impl PeerView {
    /// Project WordCraft's UI state into the fields the hub's `UiState` carries.
    ///
    /// The selection is bounded (§2.12: at most 10,000 ids) by the core's sanitizer, not here —
    /// the app reports what it truly has and the hub enforces the budget.
    #[must_use]
    pub fn from_ui(ui: &UiState) -> Self {
        Self { cursor: ui.cursor, page: ui.page, selection: ui.selection.clone(), viewport: ui.viewport, editing_text: ui.editing_text }
    }

    /// Whether this peer shows anything on `page` at all (the page bar always shows everyone;
    /// the margin badges only the peers on the same page).
    #[must_use]
    pub fn on_page(&self, page: u64) -> bool {
        self.page == Some(page)
    }
}
