//! The document knowledge the hub validates against (spec §3.6 phase A: `DocSchema`): which
//! kinds exist, which parents accept children, and the document-level entry lists that sync per
//! entry with their order.
//!
//! WordCraft's entry lists are the style sheet (keyed by stable style id) and the comments map
//! (keyed by `u32`). The comments map is a `BTreeMap`, so its iteration order is its key order —
//! deterministic everywhere — and it deliberately has **no** order property to sync: a redundant
//! one would be another way to disagree. The style sheet is a `Vec`, a real order, and syncs it.

use craft_collab::adapter::ValidationCtx;
use craft_collab::change::{Change, EntryId};
use craft_collab::limits::Limits;
use craft_collab::value::Value;
use wordcraft_doc::{Comment, Document, Style};

use crate::props;

/// The wordcraft document schema.
#[derive(Debug, Default)]
pub struct WordSchema {
    limits: Limits,
}

impl craft_collab::adapter::DocSchema for WordSchema {
    fn validate(&self, change: &Change, ctx: &ValidationCtx<'_>) -> Result<(), String> {
        match change {
            Change::Create { kind, parent, props, .. } => {
                if !crate::KINDS.contains(&kind.as_ref()) {
                    return Err(format!("unknown kind '{kind}'"));
                }
                if *parent == crate::ROOT {
                    if kind.as_ref() != "story" {
                        return Err(format!("'{kind}' does not attach to the file root; only stories do"));
                    }
                } else {
                    let p = ctx.replica.node(*parent).ok_or_else(|| format!("unknown parent {parent:?}"))?;
                    if p.kind.as_ref() != "story" {
                        return Err(format!("a '{}' accepts no children", p.kind));
                    }
                }
                let table = match kind.as_ref() {
                    "paragraph" => props::PARA_PROPS,
                    "table" => props::TABLE_PROPS,
                    _ => props::STORY_PROPS,
                };
                for (p, _) in props {
                    if p.as_ref() != "parent" && p.as_ref() != "position" && !table.contains(&p.as_ref()) {
                        return Err(format!("unknown property '{p}' for a {kind}"));
                    }
                }
                Ok(())
            }
            Change::Set { id, prop, .. } => {
                if prop.as_ref() == "parent" || prop.as_ref() == "position" {
                    return Ok(());
                }
                let node = ctx.replica.node(*id);
                let table = match node.map(|n| n.kind.as_ref()) {
                    Some("paragraph") => props::PARA_PROPS,
                    Some("table") => props::TABLE_PROPS,
                    Some("story") => props::STORY_PROPS,
                    // An unknown target's props are checked leniently against the union: the
                    // core's own existence checks refuse the change before this matters.
                    _ => return Ok(()),
                };
                if !table.contains(&prop.as_ref()) {
                    return Err(format!("unknown property '{prop}'"));
                }
                Ok(())
            }
            Change::Delete { .. } => Ok(()),
            Change::SetDoc { prop, value } => {
                if props::DOC_PROPS.contains(&prop.as_ref()) || is_order_prop(prop) {
                    return Ok(());
                }
                Err(format!("unknown document property '{prop}' (value {value:?})"))
            }
            Change::Put { entry, .. } | Change::Remove { entry } => {
                if entry_kind(&entry.0).is_some() {
                    Ok(())
                } else {
                    Err(format!("unknown entry kind in '{entry:?}'"))
                }
            }
        }
    }

    fn recompute(&self, _touched: &[craft_collab::ids::NodeId]) -> Vec<Change> {
        // WordCraft derives nothing automatically today: field updates, TOC rebuilds and layout
        // are explicit commands the author runs, not properties the document recomputes on its
        // own — so there is no author-only recompute to compose (spec §5.7's map decides:
        // "commands, not derived state"). When a derived property lands, its recompute belongs
        // here and nowhere else.
        Vec::new()
    }

    fn limits(&self) -> &Limits {
        &self.limits
    }
}

/// Which list an entry id belongs to.
fn entry_kind(entry: &str) -> Option<&'static str> {
    if entry.starts_with("Style:") {
        Some("style")
    } else if entry.starts_with("Comment:") {
        Some("comment")
    } else {
        None
    }
}

/// Whether `prop` is one of the per-list order properties. Only the style sheet carries one —
/// see the module docs for why the comments map does not.
fn is_order_prop(prop: &str) -> bool {
    matches!(prop, "styleOrder")
}

/// Upsert one document-level entry; wrong shapes are errors, never panics.
pub fn put_entry(doc: &mut Document, entry: &EntryId, value: &Value) -> Result<(), String> {
    let json = value.to_json();
    let key = entry.0.as_ref();
    match entry_kind(key) {
        Some("style") => {
            let style: Style = serde_json::from_value(json).map_err(|e| e.to_string())?;
            let want = key.strip_prefix("Style:").unwrap_or(key);
            match doc.styles.styles.iter().position(|s| s.id == want) {
                Some(i) => doc.styles.styles[i] = style,
                None => doc.styles.styles.push(style),
            }
            Ok(())
        }
        Some("comment") => {
            let comment: Comment = serde_json::from_value(json).map_err(|e| e.to_string())?;
            let n: u32 = key.strip_prefix("Comment:").and_then(|t| t.parse().ok()).ok_or_else(|| format!("malformed comment id '{key}'"))?;
            doc.comments.insert(n, comment);
            Ok(())
        }
        _ => Err(format!("unknown entry kind in '{key}'")),
    }
}

/// Remove one document-level entry (absent entries are fine — the tolerant rule).
pub fn remove_entry(doc: &mut Document, entry: &EntryId) -> Result<(), String> {
    let key = entry.0.as_ref();
    match entry_kind(key) {
        Some("style") => {
            let want = key.strip_prefix("Style:").unwrap_or(key);
            doc.styles.styles.retain(|s| s.id != want);
            Ok(())
        }
        Some("comment") => {
            let n: u32 = key.strip_prefix("Comment:").and_then(|t| t.parse().ok()).ok_or_else(|| format!("malformed comment id '{key}'"))?;
            doc.comments.remove(&n);
            Ok(())
        }
        _ => Err(format!("unknown entry kind in '{key}'")),
    }
}

/// Reorder the style sheet from the order property's value (styles the array misses keep their
/// relative order at the end — lenient, like the client replay).
pub fn set_doc_order(doc: &mut Document, prop: &str, value: &Value) -> Option<Result<(), String>> {
    let json = value.to_json();
    match prop {
        "styleOrder" => {
            let order: Vec<String> = match serde_json::from_value(json) {
                Ok(v) => v,
                Err(e) => return Some(Err(e.to_string())),
            };
            let mut decorated: Vec<(usize, Style)> = std::mem::take(&mut doc.styles.styles).into_iter().map(|s| (order.len() + 1, s)).collect();
            for (slot, style) in decorated.iter_mut() {
                if let Some(i) = order.iter().position(|o| *o == style.id) {
                    *slot = i;
                }
            }
            decorated.sort_by_key(|(slot, _)| *slot);
            doc.styles.styles = decorated.into_iter().map(|(_, s)| s).collect();
            Some(Ok(()))
        }
        _ => None,
    }
}

/// The document-level entries of `doc` in order, as `(entry id, value)` pairs for the view.
pub fn entry_values(doc: &Document) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for s in &doc.styles.styles {
        if let Some(v) = serde_json::to_value(s).ok().map(Value::Json) {
            out.push((format!("Style:{}", s.id), v));
        }
    }
    for (n, c) in &doc.comments {
        if let Some(v) = serde_json::to_value(c).ok().map(Value::Json) {
            out.push((format!("Comment:{n}"), v));
        }
    }
    out
}

/// Diff the document-level lists between two documents: per-entry `Put`/`Remove` plus one order
/// property for the style sheet when its order changed.
pub fn diff_entries(before: &Document, after: &Document) -> Vec<Change> {
    let mut out = Vec::new();
    diff_styles(before, after, &mut out);
    diff_comments(before, after, &mut out);
    out
}

/// Diff the style sheet: value `Put`s for new or changed styles, `Remove`s for gone ones, and
/// one `SetDoc` when the order changed.
fn diff_styles(before: &Document, after: &Document, out: &mut Vec<Change>) {
    let (b, a) = (&before.styles.styles, &after.styles.styles);
    let b_keys: Vec<String> = b.iter().map(|s| s.id.clone()).collect();
    let a_keys: Vec<String> = a.iter().map(|s| s.id.clone()).collect();
    if b_keys != a_keys {
        out.push(Change::SetDoc { prop: "styleOrder".into(), value: Value::Json(serde_json::json!(a_keys)) });
    }
    for style in a {
        let same = b.iter().any(|bs| bs.id == style.id && bs == style);
        if !same && let Some(v) = serde_json::to_value(style).ok().map(Value::Json) {
            out.push(Change::Put { entry: EntryId(format!("Style:{}", style.id).into()), value: v });
        }
    }
    for style in b {
        if !a_keys.contains(&style.id) {
            out.push(Change::Remove { entry: EntryId(format!("Style:{}", style.id).into()) });
        }
    }
}

/// Diff the comments map: value `Put`s and `Remove`s. The map's order is its key order, so
/// nothing else rides (see the module docs).
fn diff_comments(before: &Document, after: &Document, out: &mut Vec<Change>) {
    for (n, c) in &after.comments {
        if before.comments.get(n) != Some(c)
            && let Some(v) = serde_json::to_value(c).ok().map(Value::Json)
        {
            out.push(Change::Put { entry: EntryId(format!("Comment:{n}").into()), value: v });
        }
    }
    for n in before.comments.keys() {
        if !after.comments.contains_key(n) {
            out.push(Change::Remove { entry: EntryId(format!("Comment:{n}").into()) });
        }
    }
}
