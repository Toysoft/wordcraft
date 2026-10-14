//! # wordcraft-hub — WordCraft's CraftHub client path
//!
//! The [`HubAdapter`] that maps WordCraft's [`Document`](wordcraft_doc::Document) onto
//! `craft-collab`'s generalized object/property model (the CraftHub specification §3.6 phase B,
//! §18.4 row 2). WordCraft is the first real `textcrdt` beneficiary: the paragraph `text`
//! property opts into RGA (spec §6.6), so long concurrent writing inside one paragraph merges
//! character-level — both authors' sentences survive — instead of the whole-string
//! last-writer-wins that loses one of them at every save.
//!
//! ## The honest map (what syncs and how)
//!
//! WordCraft's real model is stories of blocks: a `body` plus secondary `parts` (headers,
//! footers, notes, comments, text boxes), each holding paragraphs and tables; a paragraph is
//! one `String` with formatting runs and inline objects. That maps onto the object tree as:
//!
//! * the **root** holds one `story` object per story — the body first, then each part by its
//!   `u32` key. A part's `partKind`, `textBody` and `partId` ride as properties.
//! * a story holds `paragraph` and `table` objects, ordered by fractional sibling keys.
//! * a **paragraph** syncs `text` (RGA — see below), `runs`, `props`, `mark`, `objects` and
//!   `section`, each a property with the compile-time guard that a new field must decide how
//!   it syncs. `Paragraph::rev` is a process-local cache counter and never syncs.
//! * a **table** syncs its `props`, `grid` (column widths), `rows` (the grid with every cell's
//!   blocks) and `controls` as one last-writer-wins value each — the same decision the pilot
//!   made for FigCraft's big list-valued fields. Cell paragraphs therefore ride inside the
//!   table value; splitting them out as child objects is the follow-up once the session wiring
//!   has validated the paragraph path (WordCraft's model gives rows and cells no identity of
//!   their own to hang ids on — that migration lands with the file format, not under a flag).
//! * document-level scalars (`settings`, `core`, `lastSection`, `numbering`, `sources`,
//!   `customProps`, `docDefaults`) sync as whole-value LWW; the **style sheet** and the
//!   **comments map** sync per entry with the style sheet's order. `media` and `passthrough`
//!   never sync: media bytes ride the asset channel, never a batch (spec §3.5), and passthrough
//!   parts are source-format round-trip data the local file owns.
//!
//! ## Block identity (the honest constraint)
//!
//! WordCraft's blocks have no persistent ids — a block is an `Arc<Block>` at a path. The
//! adapter therefore owns the identity, exactly where the pilot's adapter owns the sibling
//! keys: at first sync it assigns ids by document walk (client space 0, the file-id space, so
//! two clients opening the same file compute the same ids — including the RGA element ids of
//! every paragraph's initial text); after that it tracks blocks by `Arc` identity across
//! document clones (edits copy only what they touch, `Arc::make_mut`, so unchanged blocks stay
//! pointer-equal between the confirmed and current state), pairs edited-in-place blocks
//! positionally between the unchanged anchors, and mints ids in the local client's space for
//! genuinely new blocks. Moves detected by pointer identity keep their id and get a fresh
//! sibling key; a block's story is fixed at creation.
//!
//! ## The RGA text discipline
//!
//! The adapter keeps one [`TextDoc`](craft_collab::text::TextDoc) per paragraph beside the
//! document — the state v2's `Replica` kept for positions, kept here for text. The invariant:
//! the doc's rendered text equals the paragraph's `text` field at every sync point. A local
//! edit shows up as a prefix/suffix delta, composed through `local_insert`/`local_delete` into
//! one `Set` on the `text` property whose value is a [`Value::Text`] delta (just the new and
//! tombstoned elements, anchored) — edit-sized, riding the ordinary batch (spec §6.6 rule 2).
//! A remote delta merges element-wise, idempotently; the paragraph is then rewritten from the
//! merged render, the runs are repaired to cover it, and the inline objects are realigned to
//! the object-replacement characters (a delta that adds markers without its `objects` set is
//! padded with opaque placeholders — tolerance, never a crash). Everything else about the
//! paragraph stays last-writer-wins, per §6.6: run boundaries (formatting spans) resolve LWW
//! even when the characters under them merge.
//!
//! ## Comment anchors are ranges
//!
//! A WordCraft comment lives in three places: the `comments` map entry (author, date,
//! resolved, the part holding its text), the `CommentStart`/`CommentEnd` markers in the body
//! text (inline objects), and the part story with the comment's own paragraphs. All three
//! sync. [`WordcraftHub::comment_anchor`] derives the margin-highlight anchor — a text
//! **range** `(node, start..end)`, possibly spanning paragraphs — from the markers, so the
//! reviewing side can highlight exactly what the author marked, not a point.
//!
//! What deliberately does **not** live here: networking, sessions, the UI. The session wiring
//! (a `NetLink` speaking protocol v3, installed by the engine behind the app's `crafthub`
//! feature flag) lands as the follow-up once this contract is reviewed; nothing here changes
//! any user-visible behavior.

pub mod commands;
pub mod deeplink;
pub mod presence;
pub mod props;
pub mod schema;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use craft_collab::adapter::{ApplyReport, DocumentView, HubAdapter, UiState, ViewNode};
use craft_collab::change::Change;
use craft_collab::frac;
use craft_collab::ids::{Allocator, NodeId as HubId};
use craft_collab::text::{TextDoc, TextOp};
use craft_collab::value::Value;
use wordcraft_doc::{Block, Document, InlineObject, Paragraph, Part, PartKind, Path, Run, StoryRef, Table, para::OBJ, para_block};

/// The root every story hangs under (space 0, counter 0 — never a real id: ids allocate above
/// it, and `0` is the core's root marker).
pub const ROOT: HubId = HubId(0);

/// The object kinds: the two block kinds plus the story container.
pub const KINDS: &[&str] = &["story", "paragraph", "table"];

/// The adapter: block identity, sibling keys, the RGA text state and the presence projection
/// in one value.
///
/// The state sits behind a mutex so the adapter stays shareable; a session owns one adapter
/// per open document and the family's document discipline is single-threaded per session, so
/// contention is not a concern — correctness is.
pub struct WordcraftHub {
    schema: schema::WordSchema,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// This client's id space (granted by the server in a session; the tests use 1 and 2).
    space: u32,
    /// The id allocator (per-space counters; space 0 is the shared file-id space).
    alloc: Allocator,
    /// Sibling position keys by hub id: stories under the root, blocks under their story.
    positions: BTreeMap<HubId, String>,
    /// The RGA text state of every paragraph the hub has synced (spec §6.6).
    texts: HashMap<HubId, TextDoc>,
    /// Block identity: the hub id of a block's `Arc` pointer. The map in [`Inner::arcs`] holds
    /// the `Arc` alive, so a pointer can never be reused while it is a key.
    by_ptr: HashMap<usize, HubId>,
    /// The `Arc` versions seen per hub id: a paragraph edited in place gets a fresh `Arc`, and
    /// both the confirmed and the current document stay resolvable until their holders drop.
    arcs: HashMap<HubId, Vec<Arc<Block>>>,
    /// Story identity: the hub id of the body and of each part key.
    story_ids: BTreeMap<StoryRef, HubId>,
    /// The reverse map: a story hub id to its story reference.
    story_of: HashMap<HubId, StoryRef>,
}

/// One story's resolved assignment: the story's hub id and its blocks' `(id, Arc)` pairs.
struct StoryAssign {
    story: StoryRef,
    id: HubId,
    blocks: Vec<(HubId, Arc<Block>)>,
}

/// A comment's margin anchor: a text range, possibly spanning paragraphs (§18.4 row 2 —
/// anchors are ranges, margin-highlight style, not points). Offsets are byte offsets into the
/// paragraph's text, the same coordinate the markers live in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommentAnchor {
    /// The paragraph and offset of the first covered character.
    pub start: (HubId, usize),
    /// The paragraph and offset after the last covered character (equal to `start` for an
    /// empty highlight).
    pub end: (HubId, usize),
}

impl CommentAnchor {
    /// Whether the anchor covers a range inside one paragraph (the common case).
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.start.0 == self.end.0
    }
}

impl WordcraftHub {
    /// A fresh adapter for client space 1 (the tests' convention; a session installs the
    /// server-granted space instead).
    #[must_use]
    pub fn new() -> Self {
        Self::with_space(1)
    }

    /// A fresh adapter speaking for client `space`.
    #[must_use]
    pub fn with_space(space: u32) -> Self {
        let mut alloc = Allocator::new();
        // Space-0 counter 0 is the root marker itself: file ids start at 1.
        alloc.note(ROOT);
        Self { schema: schema::WordSchema::default(), inner: Mutex::new(Inner { space, alloc, ..Default::default() }) }
    }

    /// The sibling key of `id`, if one has been assigned.
    #[must_use]
    pub fn position_key(&self, id: HubId) -> Option<String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).positions.get(&id).cloned()
    }

    /// The RGA text state of a paragraph, if the hub has synced one.
    #[must_use]
    pub fn paragraph_text(&self, id: HubId) -> Option<String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).texts.get(&id).map(TextDoc::render)
    }

    /// The margin anchor of comment `comment`: the text range its `CommentStart`/`CommentEnd`
    /// markers cover, derived from the real document (§18.4 row 2). `None` when the markers
    /// are absent (a comment whose marked text was deleted stays as an orphan thread, exactly
    /// like Word's own tolerance).
    #[must_use]
    pub fn comment_anchor(&self, doc: &Document, comment: u32) -> Option<CommentAnchor> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut found: Option<CommentAnchor> = None;
        for story in story_list(doc) {
            let blocks = doc.story(story)?;
            for arc in blocks {
                let Block::Para(p) = &**arc else { continue };
                let Some(id) = inner.id_of(arc) else { continue };
                for (off, obj) in p.object_offsets().into_iter().zip(&p.objects) {
                    match obj {
                        InlineObject::CommentStart { id: c } if *c == comment => {
                            let at = (off + OBJ.len_utf8()).min(p.text.len());
                            if found.is_none() {
                                found = Some(CommentAnchor { start: (id, at), end: (id, at) });
                            }
                        }
                        InlineObject::CommentEnd { id: c } if *c == comment => {
                            if let Some(anchor) = &mut found {
                                anchor.end = (id, off.max(anchor.start.1));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        found
    }

    /// The kind string of a block.
    #[must_use]
    pub fn kind_name(block: &Block) -> &'static str {
        match block {
            Block::Para(_) => "paragraph",
            Block::Table(_) => "table",
        }
    }

    /// The story list in canonical order: the body, then the parts by key.
    fn stories(doc: &Document) -> Vec<StoryRef> {
        story_list(doc)
    }

    /// Resolve a document's stories and blocks into ids, registering everything seen. Unknown
    /// blocks take ids from `file_space ? space 0 : the local space` — the first sync of a
    /// file assigns the shared file-id space so two clients opening the same file agree on
    /// every id (blocks and RGA elements alike); later unknowns are local creations.
    fn resolve(&self, doc: &Document, file_space: bool) -> Vec<StoryAssign> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for story in Self::stories(doc) {
            let id = inner.register_story(story, file_space);
            let mut blocks = Vec::new();
            if let Some(list) = doc.story(story) {
                for arc in list {
                    let id = match inner.id_of(arc) {
                        Some(id) => id,
                        None => inner.next_id(file_space),
                    };
                    inner.register_block(id, arc);
                    if let Block::Para(p) = &**arc {
                        inner.ensure_text(id, &p.text, file_space);
                    }
                    blocks.push((id, arc.clone()));
                }
            }
            out.push(StoryAssign { story, id, blocks });
        }
        // First-sync rule: keys by current order for whoever has none.
        inner.assign_missing_keys(ROOT, out.iter().map(|s| s.id));
        for assign in &out {
            inner.assign_missing_keys(assign.id, assign.blocks.iter().map(|(id, _)| *id));
        }
        out
    }

    /// Match the after document against a before assignment: pointer-identical blocks keep
    /// their ids, edited-in-place blocks pair positionally between the anchors, the rest are
    /// new (local ids) or gone. Returns the after assignment plus, per after block, the before
    /// block it pairs with (for field diffs).
    fn match_after(&self, doc: &Document, before: &[StoryAssign]) -> Vec<(StoryAssign, Vec<Option<(HubId, Arc<Block>)>>)> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for story in Self::stories(doc) {
            let id = inner.register_story(story, false);
            let after_list: Vec<Arc<Block>> = doc.story(story).map_or_else(Vec::new, |b| b.to_vec());
            let before_blocks = before.iter().find(|s| s.story == story).map_or(&[][..], |s| &s.blocks);
            // Phase 1: pointer-identical blocks (Arc identity survives clones of the document).
            let mut consumed = vec![false; before_blocks.len()];
            let mut matched: Vec<Option<(HubId, Arc<Block>)>> = vec![None; after_list.len()];
            for (ai, arc) in after_list.iter().enumerate() {
                if let Some(id) = inner.id_of(arc) {
                    for (bi, (bid, _barc)) in before_blocks.iter().enumerate() {
                        if !consumed[bi] && *bid == id {
                            consumed[bi] = true;
                            matched[ai] = Some((*bid, before_blocks[bi].1.clone()));
                            break;
                        }
                    }
                }
            }
            // Phase 2: between consecutive anchors (in both orders), pair the leftovers
            // positionally — the run between two unchanged neighbours is an in-place edit.
            let mut prev_a = 0;
            let mut prev_b = 0;
            for ai in 0..=after_list.len() {
                if ai < after_list.len() && matched[ai].is_none() {
                    continue;
                }
                let b_end = if ai == after_list.len() {
                    before_blocks.len()
                } else {
                    matched[ai].as_ref().map_or(prev_b, |(id, _)| before_blocks.iter().position(|(bid, _)| *bid == *id).map_or(prev_b, |p| p + 1))
                };
                if b_end > prev_b {
                    let before_free: Vec<usize> = (prev_b..b_end).filter(|bi| !consumed[*bi]).collect();
                    let after_free: Vec<usize> = (prev_a..ai).filter(|a| matched[*a].is_none()).collect();
                    for (free_a, free_b) in after_free.into_iter().zip(before_free) {
                        // Only pair same-kind blocks: a paragraph replaced by a table is a
                        // delete plus a create, not an edit.
                        let kinds_match = before_blocks[free_b].1.as_para().is_some() == after_list[free_a].as_para().is_some();
                        if kinds_match {
                            matched[free_a] = Some((before_blocks[free_b].0, before_blocks[free_b].1.clone()));
                            consumed[free_b] = true;
                        }
                    }
                }
                prev_a = ai + 1;
                prev_b = b_end;
            }
            // Register the after side: matched blocks keep their id, new blocks mint one.
            let mut blocks = Vec::new();
            for (ai, arc) in after_list.into_iter().enumerate() {
                let id = match &matched[ai] {
                    Some((id, _)) => *id,
                    None => inner.next_id(false),
                };
                inner.register_block(id, &arc);
                if let Block::Para(p) = &*arc {
                    inner.ensure_text(id, &p.text, false);
                }
                blocks.push((id, arc));
            }
            out.push((StoryAssign { story, id, blocks }, matched));
        }
        out
    }

    /// Diff one parent's child order between two documents (the pilot's LIS discipline: the
    /// longest rising run of old keys keeps its keys, only the moved and inserted children get
    /// fresh ones). Emits `Set(position)` changes into `out`.
    fn diff_order(&self, old: Vec<HubId>, new: Vec<HubId>, out: &mut Vec<Change>) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if old == new {
            return;
        }
        inner.assign_missing_keys(ROOT, old.iter().copied());
        let has_key = |i: &HubId| inner.positions.contains_key(i);
        let old_member = |i: &HubId| old.contains(i);
        let n = new.len();
        let mut len = vec![0usize; n];
        let mut prev = vec![usize::MAX; n];
        let mut best_end = None;
        let mut best_len = 0usize;
        for i in 0..n {
            if has_key(&new[i]) && old_member(&new[i]) {
                len[i] = 1;
                for j in 0..i {
                    if has_key(&new[j])
                        && old_member(&new[j])
                        && inner.positions.get(&new[j]).is_some_and(|a| inner.positions.get(&new[i]).is_some_and(|b| a < b))
                        && len[j] + 1 > len[i]
                    {
                        len[i] = len[j] + 1;
                        prev[i] = j;
                    }
                }
                if len[i] > best_len {
                    best_len = len[i];
                    best_end = Some(i);
                }
            }
        }
        let mut kept = vec![false; n];
        let mut walk = best_end;
        while let Some(i) = walk {
            kept[i] = true;
            walk = if prev[i] == usize::MAX { None } else { Some(prev[i]) };
        }
        fn flush(
            pending: &mut Vec<HubId>,
            prev_key: Option<&str>,
            next_key: Option<&str>,
            positions: &mut BTreeMap<HubId, String>,
            out: &mut Vec<Change>,
        ) {
            if pending.is_empty() {
                return;
            }
            let count = pending.len();
            if let Ok(fresh) = frac::n_between(prev_key, next_key, count) {
                for (id, key) in pending.drain(..).zip(fresh) {
                    positions.insert(id, key.clone());
                    out.push(Change::Set { id, prop: Arc::from("position"), value: Value::Json(serde_json::Value::String(key)) });
                }
            }
        }
        let mut last_kept_key: Option<String> = None;
        let mut pending: Vec<HubId> = Vec::new();
        for (i, c) in new.iter().enumerate() {
            if kept[i] {
                let next_key = inner.positions.get(c).cloned();
                flush(&mut pending, last_kept_key.as_deref(), next_key.as_deref(), &mut inner.positions, out);
                last_kept_key = inner.positions.get(c).cloned();
            } else {
                pending.push(*c);
            }
        }
        flush(&mut pending, last_kept_key.as_deref(), None, &mut inner.positions, out);
    }
}

impl Default for WordcraftHub {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    fn is_fresh(&self) -> bool {
        self.by_ptr.is_empty() && self.story_ids.is_empty()
    }

    fn next_id(&mut self, file_space: bool) -> HubId {
        self.alloc.alloc(if file_space { 0 } else { self.space })
    }

    fn register_story(&mut self, story: StoryRef, file_space: bool) -> HubId {
        if let Some(id) = self.story_ids.get(&story) {
            return *id;
        }
        let id = self.next_id(file_space);
        self.story_ids.insert(story, id);
        self.story_of.insert(id, story);
        id
    }

    fn register_block(&mut self, id: HubId, arc: &Arc<Block>) {
        let ptr = Arc::as_ptr(arc) as usize;
        self.by_ptr.insert(ptr, id);
        let slots = self.arcs.entry(id).or_default();
        slots.push(arc.clone());
        // Prune the versions nobody else holds (their pointers can never be looked up again,
        // and keeping them mapped would risk a reused address).
        if slots.len() > 1 {
            slots.retain(|a| Arc::strong_count(a) > 1 || Arc::as_ptr(a) as usize == ptr);
            for a in slots.iter() {
                self.by_ptr.insert(Arc::as_ptr(a) as usize, id);
            }
        }
    }

    fn id_of(&self, arc: &Arc<Block>) -> Option<HubId> {
        self.by_ptr.get(&(Arc::as_ptr(arc) as usize)).copied()
    }

    /// Find a block by hub id in the document (fresh scan: never trusts a stale path).
    fn find_block(&self, doc: &Document, id: HubId) -> Option<(StoryRef, usize)> {
        for story in story_list(doc) {
            if let Some(blocks) = doc.story(story) {
                for (i, arc) in blocks.iter().enumerate() {
                    if self.id_of(arc) == Some(id) {
                        return Some((story, i));
                    }
                }
            }
        }
        None
    }

    /// The RGA state of paragraph `id`, built from its current text if the hub has none.
    fn ensure_text(&mut self, id: HubId, text: &str, file_space: bool) {
        if !self.texts.contains_key(&id) {
            let mut doc = TextDoc::new();
            let _ = doc.local_insert(0, text, &mut self.alloc, if file_space { 0 } else { self.space });
            self.texts.insert(id, doc);
        }
    }

    /// First-sync rule: `children` without keys take them by current order.
    fn assign_missing_keys<I: Iterator<Item = HubId>>(&mut self, _parent: HubId, children: I) {
        let missing: Vec<HubId> = children.filter(|c| !self.positions.contains_key(c)).collect();
        if !missing.is_empty()
            && let Ok(keys) = frac::n_between(None, None, missing.len())
        {
            for (id, key) in missing.into_iter().zip(keys) {
                self.positions.insert(id, key);
            }
        }
    }

    /// Drop a story's synced state (its blocks' keys and text) when the story is deleted.
    fn forget_story(&mut self, blocks: &[HubId], story_id: HubId) {
        for id in blocks {
            self.positions.remove(id);
            self.texts.remove(id);
        }
        self.positions.remove(&story_id);
    }
}

/// The canonical story list: the body first, then the parts by key.
fn story_list(doc: &Document) -> Vec<StoryRef> {
    let mut out = vec![StoryRef::Body];
    out.extend(doc.parts.keys().map(|k| StoryRef::Part(*k)));
    out
}

/// The insertion index for `(key, id)` among `siblings` ordered by `(key, id)`.
fn insertion_index(inner: &Inner, doc: &Document, story: StoryRef, id: HubId, key: &str) -> usize {
    let mut rank = 0usize;
    if let Some(blocks) = doc.story(story) {
        for arc in blocks {
            if let Some(sid) = inner.id_of(arc) {
                let sk = inner.positions.get(&sid).cloned().unwrap_or_default();
                if (sk, sid) < (key.to_owned(), id) {
                    rank += 1;
                }
            }
        }
    }
    rank
}

/// Compute the local edit between two paragraph texts and fold it into the RGA state, returning
/// the delta as a `Value::Text` (just the new and tombstoned elements — spec §6.6 rule 2).
/// `None` when the text didn't change.
fn text_delta(td: &mut TextDoc, after: &str, alloc: &mut Allocator, space: u32) -> Option<Value> {
    let before = td.render();
    if before == after {
        return None;
    }
    // Common prefix and suffix, snapped to character boundaries.
    let b = before.as_bytes();
    let a = after.as_bytes();
    let mut p = 0;
    while p < b.len() && p < a.len() && b[p] == a[p] {
        p += 1;
    }
    while p > 0 && (!before.is_char_boundary(p) || !after.is_char_boundary(p)) {
        p -= 1;
    }
    let mut s = 0;
    while s < before.len() - p && s < after.len() - p && b[b.len() - 1 - s] == a[a.len() - 1 - s] {
        s += 1;
    }
    while s > 0 && (!before.is_char_boundary(before.len() - s) || !after.is_char_boundary(after.len() - s)) {
        s -= 1;
    }
    let del_bytes = before.len() - p - s;
    let ins = &after[p..after.len() - s];
    let mut ops = Vec::new();
    if del_bytes > 0 {
        let start = before[..p].chars().count();
        let len = before[p..before.len() - s].chars().count();
        ops.extend(td.local_delete(start, len));
    }
    if !ins.is_empty() {
        // The insert position is `p` characters into the post-delete render too (the prefix
        // survives the deletes), and the ids come from the client's own space via the shared
        // allocator — never a fresh space-0 allocator, whose ids would collide with the root
        // marker, the first-sync walk ids, and every other delta's ids.
        let at = before[..p].chars().count();
        ops.extend(td.local_insert(at, ins, alloc, space));
    }
    compose_delta(td, &ops)
}

/// Build the wire delta from text operations: a `TextDoc` holding exactly the inserted elements
/// and the tombstoned ones, each with its full anchor (the core's `TextDoc::delta` — a bare
/// `Del` op applied to a fresh doc is a silent no-op, so the tombstones must ride explicitly).
fn compose_delta(td: &TextDoc, ops: &[TextOp]) -> Option<Value> {
    if ops.is_empty() {
        return None;
    }
    Some(Value::Text(td.delta(ops)))
}

/// Merge a received delta into a paragraph's RGA state, element-wise and idempotently. The
/// delta's elements ride its serde form (the same form `Value::Text` carries on the wire), so
/// the merge needs no private access into the core's text module.
fn merge_delta(td: &mut TextDoc, delta: &TextDoc) {
    let Ok(json) = serde_json::to_value(delta) else { return };
    let Some(elems) = json.get("elems").and_then(serde_json::Value::as_array) else { return };
    for elem in elems {
        let id = elem.get("id").and_then(serde_json::Value::as_u64).map(HubId);
        let Some(id) = id else { continue };
        let tombstone = elem.get("tombstone").and_then(serde_json::Value::as_bool).unwrap_or(false);
        if tombstone {
            td.apply(&TextOp::Del { id });
            continue;
        }
        let after = match elem.get("after") {
            Some(serde_json::Value::Null) | None => None,
            Some(v) => v.as_u64().map(HubId),
        };
        let ch = elem.get("ch").and_then(serde_json::Value::as_str).and_then(|s| s.chars().next());
        if let Some(ch) = ch {
            td.apply(&TextOp::Ins { after, id, ch });
        }
    }
}

/// Repair a paragraph after its text changed underneath it: the runs must cover the text
/// exactly, and every object-replacement character must have its inline object. Wrong shapes
/// are repaired, never panicked on.
fn repair_paragraph(p: &mut Paragraph) {
    let len = p.text.len();
    let mut total: usize = p.runs.iter().map(|r| r.len).sum();
    if total != len {
        if p.runs.is_empty() {
            if len > 0 {
                p.runs.push(Run { len, props: p.mark.clone() });
            }
        } else {
            while total > len {
                match p.runs.last_mut() {
                    Some(last) if last.len > total - len => {
                        last.len -= total - len;
                        total = len;
                    }
                    Some(last) => {
                        total -= last.len;
                        p.runs.pop();
                    }
                    None => break,
                }
            }
            if total < len
                && let Some(last) = p.runs.last_mut()
            {
                last.len += len - total;
            }
        }
        p.runs.retain(|r| r.len > 0);
        if p.runs.is_empty() && len > 0 {
            p.runs.push(Run { len, props: p.mark.clone() });
        }
    }
    let objs = p.text.chars().filter(|c| *c == OBJ).count();
    if p.objects.len() > objs {
        p.objects.truncate(objs);
    }
    while p.objects.len() < objs {
        // A delta that added markers without its `objects` set (a hostile or hand-crafted
        // batch): pad with opaque placeholders so the document stays well-formed.
        p.objects.push(InlineObject::Opaque { format: "crafthub".into(), xml: String::new(), text: String::new() });
    }
    p.touch();
}

/// The story-level property accessors (the part properties plus the part key, which is the
/// story's file identity — carried so a remote apply can use the author's key, read-only).
mod story_props {
    use super::*;

    /// Every story property that syncs.
    pub const STORY_PROPS: &[&str] = &["partKind", "textBody", "partId"];

    /// The value of a story property; the body story carries none of them.
    pub fn value(key: Option<u32>, part: &Part, prop: &str) -> Option<Value> {
        match prop {
            "partKind" => Some(Value::Json(serde_json::to_value(part.kind).ok()?)),
            "textBody" => Some(Value::Json(serde_json::to_value(part.body).ok()?)),
            "partId" => key.map(|k| Value::Json(serde_json::json!(k))),
            _ => None,
        }
    }

    /// Whether a story property differs; the body story has nothing to diff.
    pub fn differs(a_key: Option<u32>, a: &Part, b_key: Option<u32>, b: &Part, prop: &str) -> Option<bool> {
        match prop {
            "partKind" => Some(a.kind != b.kind),
            "textBody" => Some(a.body != b.body),
            "partId" => Some(a_key != b_key),
            _ => None,
        }
    }

    /// Write a story property on a part; `partId` is decided at creation and never rewritten
    /// (returned as an error so the change counts as skipped, not silently accepted).
    pub fn set(part: &mut Part, prop: &str, value: &Value) -> Option<Result<(), String>> {
        match prop {
            "partKind" => Some(serde_json::from_value::<PartKind>(value.to_json()).map(|v| part.kind = v).map_err(|e| e.to_string())),
            "textBody" => Some(serde_json::from_value(value.to_json()).map(|v| part.body = v).map_err(|e| e.to_string())),
            "partId" => Some(Err("the part key is fixed at creation".into())),
            _ => None,
        }
    }

    /// The pilot's compile-time guard, hand-written for the part (its `blocks` are the story's
    /// children, not a property).
    #[allow(dead_code)]
    fn every_part_field_is_a_property(p: &Part) {
        let Part { kind: _, blocks: _, body: _ } = p;
    }
}

impl HubAdapter for WordcraftHub {
    type Doc = Document;

    fn view(&self, doc: &Document) -> DocumentView {
        let fresh = self.inner.lock().unwrap_or_else(|e| e.into_inner()).is_fresh();
        let assigns = self.resolve(doc, fresh);
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut nodes = Vec::new();
        for assign in &assigns {
            let props = match assign.story {
                StoryRef::Body => Vec::new(),
                StoryRef::Part(key) => {
                    let Some(part) = doc.parts.get(&key) else { continue };
                    story_props::STORY_PROPS.iter().filter_map(|p| story_props::value(Some(key), part, p).map(|v| (Arc::from(*p), v))).collect()
                }
            };
            nodes.push(ViewNode {
                id: assign.id,
                kind: Arc::from("story"),
                parent: ROOT,
                position: inner.positions.get(&assign.id).cloned().unwrap_or_default(),
                props,
            });
            for (id, arc) in &assign.blocks {
                let props = match &**arc {
                    Block::Para(p) => props::PARA_PROPS
                        .iter()
                        .filter_map(|prop| {
                            if *prop == "text" {
                                inner.texts.get(id).cloned().map(Value::Text).map(|v| (Arc::from("text"), v))
                            } else {
                                props::para_value(p, prop).map(|v| (Arc::from(*prop), v))
                            }
                        })
                        .collect(),
                    Block::Table(t) => {
                        props::TABLE_PROPS.iter().filter_map(|prop| props::table_value(t, prop).map(|v| (Arc::from(*prop), v))).collect()
                    }
                };
                nodes.push(ViewNode {
                    id: *id,
                    kind: Arc::from(Self::kind_name(arc)),
                    parent: assign.id,
                    position: inner.positions.get(id).cloned().unwrap_or_default(),
                    props,
                });
            }
        }
        let entries = schema::entry_values(doc);
        let doc_props = props::DOC_PROPS.iter().filter_map(|p| props::doc_prop_value(doc, p).map(|v| ((*p).to_owned(), v))).collect();
        DocumentView { nodes, entries, doc: doc_props }
    }

    fn apply(&self, doc: &mut Document, changes: &[Change]) -> ApplyReport {
        let mut report = ApplyReport::default();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for change in changes {
            match change {
                Change::Create { id, kind, parent, position, props } => match kind.as_ref() {
                    "story" => {
                        let mut part =
                            Part { kind: props::default_part_kind(), blocks: vec![para_block(Paragraph::new())], body: Default::default() };
                        for (p, v) in props {
                            if p.as_ref() == "parent" || p.as_ref() == "position" {
                                continue;
                            }
                            match story_props::set(&mut part, p, v) {
                                Some(Ok(())) | Some(Err(_)) => {}
                                None => {}
                            }
                        }
                        // The author's part key when it is free here (so comment entries that
                        // reference `Comment::part` resolve); otherwise the document allocates.
                        let key = props
                            .iter()
                            .find(|(p, _)| p.as_ref() == "partId")
                            .and_then(|(_, v)| v.to_json().as_u64())
                            .and_then(|k| u32::try_from(k).ok())
                            .filter(|k| !doc.parts.contains_key(k))
                            .unwrap_or_else(|| doc.add_part(part.kind, std::mem::take(&mut part.blocks)));
                        if key == 0 {
                            // add_part never returns 0; a zero key would collide with nothing
                            // but is not a key the model issues.
                            report.skipped += 1;
                            continue;
                        }
                        // An existing part keeps its blocks (the story's own diff owns
                        // those); only a brand-new part lands whole.
                        doc.parts.entry(key).or_insert(part);
                        inner.register_story_at(StoryRef::Part(key), *id);
                        inner.positions.insert(*id, position.clone());
                        report.applied += 1;
                    }
                    block_kind @ ("paragraph" | "table") => {
                        let Some(story) = inner.story_of.get(parent).copied() else {
                            report.skipped += 1;
                            continue;
                        };
                        let mut para = Paragraph::new();
                        let mut table = Table::default();
                        for (p, v) in props {
                            if p.as_ref() == "parent" || p.as_ref() == "position" {
                                continue;
                            }
                            let written = if block_kind == "paragraph" {
                                if p.as_ref() == "text" {
                                    adopt_text(&mut inner, *id, &mut para, v)
                                } else {
                                    matches!(props::set_para(&mut para, p, v), Some(Ok(())))
                                }
                            } else {
                                matches!(props::set_table(&mut table, p, v), Some(Ok(())))
                            };
                            if !written {
                                report.skipped += 1;
                            }
                        }
                        let block = if block_kind == "paragraph" {
                            repair_paragraph(&mut para);
                            Block::Para(para)
                        } else {
                            Block::Table(table)
                        };
                        inner.positions.insert(*id, position.clone());
                        let at = insertion_index(&inner, doc, story, *id, position);
                        if doc.insert_block(story, &Path::top(at), block).is_ok() {
                            if let Some(arc) = doc.story(story).and_then(|b| b.get(at)) {
                                inner.register_block(*id, arc);
                            }
                            report.applied += 1;
                        } else {
                            report.skipped += 1;
                        }
                    }
                    _ => report.skipped += 1,
                },
                Change::Set { id, prop, value } => {
                    if prop.as_ref() == "position" {
                        let Value::Json(serde_json::Value::String(key)) = value else {
                            report.skipped += 1;
                            continue;
                        };
                        inner.positions.insert(*id, key.clone());
                        let Some(story) = inner.story_of.get(id).copied().or_else(|| inner.find_block(doc, *id).map(|(s, _)| s)) else {
                            report.skipped += 1;
                            continue;
                        };
                        reorder_story(&inner, doc, story);
                        report.applied += 1;
                    } else if prop.as_ref() == "parent" {
                        // A block's story is fixed at creation in WordCraft's model; a story
                        // never moves either (the root order is derived from the part keys).
                        report.skipped += 1;
                    } else if let Some(story) = inner.story_of.get(id).copied() {
                        // A story-level property (part properties).
                        let StoryRef::Part(key) = story else {
                            report.skipped += 1; // the body story carries no properties
                            continue;
                        };
                        match doc.parts.get_mut(&key).map(|part| story_props::set(part, prop, value)) {
                            Some(Some(Ok(()))) => report.applied += 1,
                            _ => report.skipped += 1,
                        }
                    } else if let Some((story, idx)) = inner.find_block(doc, *id) {
                        let path = Path::top(idx);
                        let written = match doc.block(story, &path) {
                            Some(Block::Table(_)) => {
                                doc.table_mut(story, &path).is_ok_and(|t| matches!(props::set_table(t, prop, value), Some(Ok(()))))
                            }
                            _ => doc.para_mut(story, &path).is_ok_and(|p| {
                                if prop.as_ref() == "text" {
                                    apply_text(&mut inner, *id, p, value)
                                } else {
                                    matches!(props::set_para(p, prop, value), Some(Ok(())))
                                }
                            }),
                        };
                        if written {
                            if let Some(arc) = doc.story(story).and_then(|b| b.get(idx)) {
                                inner.register_block(*id, arc);
                            }
                            report.applied += 1;
                        } else {
                            report.skipped += 1;
                        }
                    } else {
                        report.skipped += 1;
                    }
                }
                Change::Delete { id } => {
                    if let Some(story) = inner.story_of.get(id).copied() {
                        let StoryRef::Part(key) = story else {
                            report.skipped += 1; // the body story is never deleted
                            continue;
                        };
                        let blocks: Vec<HubId> = doc.story(story).map(|b| b.iter().filter_map(|a| inner.id_of(a)).collect()).unwrap_or_default();
                        if doc.parts.remove(&key).is_some() {
                            inner.forget_story(&blocks, *id);
                            inner.story_ids.remove(&story);
                            inner.story_of.remove(id);
                            report.applied += 1;
                        } else {
                            report.skipped += 1;
                        }
                    } else if let Some((story, idx)) = inner.find_block(doc, *id) {
                        if doc.remove_block(story, &Path::top(idx)).is_ok() {
                            inner.positions.remove(id);
                            inner.texts.remove(id);
                            report.applied += 1;
                        } else {
                            report.skipped += 1;
                        }
                    }
                    // Deleting a deleted object does nothing (tolerance, kept).
                }
                Change::SetDoc { prop, value } => {
                    let routed = props::set_doc_prop(doc, prop, value).or_else(|| schema::set_doc_order(doc, prop, value));
                    match routed {
                        Some(Ok(())) => report.applied += 1,
                        Some(Err(_)) | None => report.skipped += 1,
                    }
                }
                Change::Put { entry, value } => match schema::put_entry(doc, entry, value) {
                    Ok(()) => report.applied += 1,
                    Err(_) => report.skipped += 1,
                },
                Change::Remove { entry } => match schema::remove_entry(doc, entry) {
                    Ok(()) => report.applied += 1,
                    Err(_) => report.skipped += 1,
                },
            }
        }
        report
    }

    fn diff(&self, before: &Document, after: &Document) -> Vec<Change> {
        let fresh = self.inner.lock().unwrap_or_else(|e| e.into_inner()).is_fresh();
        let before_assigns = self.resolve(before, fresh);
        let after_matches = self.match_after(after, &before_assigns);

        let mut sets = Vec::new();
        let mut creates = Vec::new();

        // Story-level property sets and creates.
        for (assign, _) in &after_matches {
            match assign.story {
                StoryRef::Body => {}
                StoryRef::Part(key) => {
                    let part = after.parts.get(&key);
                    let bpart = before.parts.get(&key);
                    let (Some(ap), Some(bp)) = (part, bpart) else {
                        // A story that exists after but not before is a creation; one that is
                        // gone after is a deletion and never reaches this walk.
                        if let Some(ap) = part {
                            creates.push(Change::Create {
                                id: assign.id,
                                kind: Arc::from("story"),
                                parent: ROOT,
                                position: String::new(),
                                props: story_props::STORY_PROPS
                                    .iter()
                                    .filter_map(|p| story_props::value(Some(key), ap, p).map(|v| (Arc::from(*p), v)))
                                    .collect(),
                            });
                        }
                        continue;
                    };
                    for prop in story_props::STORY_PROPS {
                        if story_props::differs(Some(key), ap, Some(key), bp, prop).unwrap_or(false)
                            && let Some(v) = story_props::value(Some(key), ap, prop)
                        {
                            sets.push(Change::Set { id: assign.id, prop: Arc::from(*prop), value: v });
                        }
                    }
                }
            }
        }

        // Block field sets and creations, per story.
        for (assign, matched) in &after_matches {
            for (ai, (id, arc)) in assign.blocks.iter().enumerate() {
                let Some((_bid, barc)) = &matched[ai] else {
                    // A creation: the whole block (the order pass below assigns its key).
                    let props = match &**arc {
                        Block::Para(p) => {
                            let text = self.inner.lock().unwrap_or_else(|e| e.into_inner()).texts.get(id).cloned();
                            props::PARA_PROPS
                                .iter()
                                .filter_map(|prop| {
                                    if *prop == "text" {
                                        text.clone().map(Value::Text).map(|v| (Arc::from("text"), v))
                                    } else {
                                        props::para_value(p, prop).map(|v| (Arc::from(*prop), v))
                                    }
                                })
                                .collect()
                        }
                        Block::Table(t) => {
                            props::TABLE_PROPS.iter().filter_map(|prop| props::table_value(t, prop).map(|v| (Arc::from(*prop), v))).collect()
                        }
                    };
                    creates.push(Change::Create {
                        id: *id,
                        kind: Arc::from(Self::kind_name(arc)),
                        parent: assign.id,
                        position: String::new(),
                        props,
                    });
                    continue;
                };
                if Arc::ptr_eq(barc, arc) {
                    continue; // structurally shared: nothing changed
                }
                match (&**barc, &**arc) {
                    (Block::Para(bp), Block::Para(ap)) => {
                        for prop in props::PARA_PROPS {
                            if *prop == "text" {
                                // The RGA path: fold the local edit into the paragraph's text
                                // state and carry just the delta (spec §6.6 rule 2). The state
                                // leaves the map for the call (the allocator lives beside it).
                                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(mut td) = inner.texts.remove(id) {
                                    let space = inner.space;
                                    let delta = text_delta(&mut td, &ap.text, &mut inner.alloc, space);
                                    inner.texts.insert(*id, td);
                                    if let Some(delta) = delta {
                                        drop(inner);
                                        sets.push(Change::Set { id: *id, prop: Arc::from("text"), value: delta });
                                    }
                                }
                            } else if props::para_differs(bp, ap, prop).unwrap_or(false)
                                && let Some(v) = props::para_value(ap, prop)
                            {
                                sets.push(Change::Set { id: *id, prop: Arc::from(*prop), value: v });
                            }
                        }
                    }
                    (Block::Table(bt), Block::Table(at)) => {
                        for prop in props::TABLE_PROPS {
                            if props::table_differs(bt, at, prop).unwrap_or(false)
                                && let Some(v) = props::table_value(at, prop)
                            {
                                sets.push(Change::Set { id: *id, prop: Arc::from(*prop), value: v });
                            }
                        }
                    }
                    // Kind changes never pair (match_after only pairs same-kind blocks).
                    _ => {}
                }
            }
        }

        // Deletions: one Delete per gone story (a subtree — its blocks need none), and one per
        // gone block in a surviving story (blocks have no children in this mapping: tables ride
        // as one value).
        let mut deletes = Vec::new();
        for assign in &before_assigns {
            match after_matches.iter().find(|(a, _)| a.story == assign.story) {
                Some((after_assign, matched)) => {
                    let consumed: Vec<bool> = matched.iter().map(|m| m.is_some()).collect();
                    for (id, _) in assign.blocks.iter() {
                        let paired = after_assign.blocks.iter().any(|(aid, _)| aid == id);
                        if !paired && (!consumed.contains(&true) || !matched_matches_id(matched, *id)) {
                            deletes.push(Change::Delete { id: *id });
                        }
                    }
                }
                None => deletes.push(Change::Delete { id: assign.id }),
            }
        }

        // Order and moves per parent: the root's stories, then every story's blocks.
        let mut order_changes = Vec::new();
        let old_root: Vec<HubId> = before_assigns.iter().map(|s| s.id).collect();
        let new_root: Vec<HubId> = after_matches.iter().map(|(a, _)| a.id).collect();
        self.diff_order(old_root, new_root, &mut order_changes);
        for (assign, _) in &after_matches {
            let old: Vec<HubId> =
                before_assigns.iter().find(|s| s.story == assign.story).map_or_else(Vec::new, |s| s.blocks.iter().map(|(id, _)| *id).collect());
            let new: Vec<HubId> = assign.blocks.iter().map(|(id, _)| *id).collect();
            self.diff_order(old, new, &mut order_changes);
        }

        // The creations' keys, now that the order pass has assigned them.
        {
            let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            for create in &mut creates {
                if let Change::Create { id, position, .. } = create {
                    *position = inner.positions.get(id).cloned().unwrap_or_default();
                }
            }
        }

        // Document-level scalars and entries.
        let mut doc_level = Vec::new();
        for prop in props::DOC_PROPS {
            if props::doc_prop_value(before, prop) != props::doc_prop_value(after, prop)
                && let Some(v) = props::doc_prop_value(after, prop)
            {
                doc_level.push(Change::SetDoc { prop: Arc::from(*prop), value: v });
            }
        }
        doc_level.extend(schema::diff_entries(before, after));

        // Batch order (§2.3): field writes, creations (stories before their blocks), moves,
        // deletes, doc level.
        let mut batch = sets;
        batch.append(&mut creates);
        batch.append(&mut order_changes);
        batch.extend(deletes);
        batch.append(&mut doc_level);
        batch
    }

    fn presence(&self, _doc: &Document, ui: &UiState) -> Option<craft_collab::presence::PresenceMsg> {
        let page = ui.page?;
        Some(craft_collab::presence::PresenceMsg {
            cursor: ui.cursor,
            page: Some(page),
            selection: ui.selection.clone(),
            viewport: ui.viewport,
            editing_text: ui.editing_text,
            playhead: None,    // WordCraft has no timeline
            track_range: None, // WordCraft has no tracks
            generating: None,  // WordCraft runs no generation jobs
        })
    }

    fn schema(&self) -> &dyn craft_collab::adapter::DocSchema {
        &self.schema
    }
}

/// Whether one of the after matches landed on before id `id`.
fn matched_matches_id(matched: &[Option<(HubId, Arc<Block>)>], id: HubId) -> bool {
    matched.iter().flatten().any(|(bid, _)| *bid == id)
}

/// Sort one story's blocks by `(key, id)` after a key change, in the document itself.
fn reorder_story(inner: &Inner, doc: &mut Document, story: StoryRef) {
    let Some(mut blocks) = doc.story(story).cloned() else { return };
    blocks.sort_by_key(|arc| {
        inner.id_of(arc).map_or_else(|| (String::new(), HubId(0)), |id| (inner.positions.get(&id).cloned().unwrap_or_default(), id))
    });
    let _ = doc.set_story(story, blocks);
}

/// Adopt the text property of a freshly created paragraph: an RGA value becomes the paragraph's
/// text state as given; a plain string builds one (and reports success either way).
fn adopt_text(inner: &mut Inner, id: HubId, para: &mut Paragraph, value: &Value) -> bool {
    match value {
        Value::Text(delta) => {
            let mut td = TextDoc::new();
            merge_delta(&mut td, delta);
            para.text = td.render();
            inner.texts.insert(id, td);
            true
        }
        Value::Json(json) => {
            if let Some(s) = json.as_str() {
                let mut td = TextDoc::new();
                let _ = td.local_insert(0, s, &mut inner.alloc, inner.space);
                para.text = td.render();
                inner.texts.insert(id, td);
                true
            } else {
                false
            }
        }
        Value::AssetRef(_) => false,
    }
}

/// Apply a text `Set`: merge the delta into the paragraph's RGA state, then rewrite the
/// paragraph from the merged render (runs repaired, objects realigned).
fn apply_text(inner: &mut Inner, id: HubId, para: &mut Paragraph, value: &Value) -> bool {
    let Value::Text(delta) = value else {
        return adopt_text_no_state(inner, id, para, value);
    };
    let mut td = match inner.texts.remove(&id) {
        Some(td) => td,
        None => {
            // The hub never synced this paragraph's text (a change for an object outside the
            // resolved state): build the base from what is here, best effort.
            let mut fresh = TextDoc::new();
            let _ = fresh.local_insert(0, &para.text, &mut inner.alloc, inner.space);
            fresh
        }
    };
    merge_delta(&mut td, delta);
    let merged = td.render();
    if merged != para.text {
        para.text = merged;
        repair_paragraph(para);
    }
    inner.texts.insert(id, td);
    true
}

/// The non-RGA fallback for a text set (a plain string replaces the state, LWW).
fn adopt_text_no_state(inner: &mut Inner, id: HubId, para: &mut Paragraph, value: &Value) -> bool {
    let Value::Json(json) = value else { return false };
    let Some(s) = json.as_str() else { return false };
    let mut td = TextDoc::new();
    let _ = td.local_insert(0, s, &mut inner.alloc, inner.space);
    para.text = td.render();
    inner.texts.insert(id, td);
    true
}

impl Inner {
    fn register_story_at(&mut self, story: StoryRef, id: HubId) {
        self.story_ids.insert(story, id);
        self.story_of.insert(id, story);
    }
}
