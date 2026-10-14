//! The synchronized property tables (spec §2.3: "every field the file saves is a property").
//!
//! WordCraft's model is not one flat node table like FigCraft's: a story holds two block kinds
//! with different fields, and the secondary stories carry part properties of their own. So this
//! module is three small tables instead of one big one — [`PARA_PROPS`] for [`Paragraph`],
//! [`TABLE_PROPS`] for [`Table`], [`STORY_PROPS`] for the part properties of a secondary story —
//! each with the pilot's compile-time guard: a struct gaining a field that doesn't sync is a
//! build error, not a silent divergence.
//!
//! The `text` property is the one that opts into RGA (spec §6.6, feature word `textcrdt`):
//! concurrent writing inside one paragraph merges character-level instead of the whole-string
//! last-writer-wins every other property uses. `Paragraph::rev` is a process-local cache
//! counter (not part of equality, not saved), so it is the one field that never syncs.

use craft_collab::Value;
use wordcraft_doc::{Paragraph, Part, PartKind, Table};

/// Declares one property table over a struct's fields: the name list, the getters/setters, the
/// differs, and the every-field-is-a-property guard.
macro_rules! props_table {
    ($ty:ident, $props:ident, $value:ident, $set:ident, $differs:ident, $guard:ident, ($($skip:ident),*), ($(($field:ident, $name:literal)),+ $(,)?)) => {
        /// Every field that syncs, by property name.
        pub const $props: &[&str] = &[$($name),+];

        /// The value of field property `prop`; `None` when `prop` is not a field.
        pub fn $value(obj: &$ty, prop: &str) -> Option<Value> {
            match prop {
                $($name => Some(Value::Json(serde_json::to_value(&obj.$field).ok()?)),)+
                _ => None,
            }
        }

        /// Write field property `prop`; `None` when `prop` is not a field.
        ///
        /// A value of the wrong shape is an error, never a panic — shipped code never crashes.
        pub fn $set(obj: &mut $ty, prop: &str, value: &Value) -> Option<Result<(), String>> {
            match prop {
                $($name => Some(
                    serde_json::from_value(value.to_json())
                        .map(|v| obj.$field = v)
                        .map_err(|e| e.to_string()),
                ),)+
                _ => None,
            }
        }

        /// Whether field property `prop` differs between `a` and `b`; `None` if not a field.
        pub fn $differs(a: &$ty, b: &$ty, prop: &str) -> Option<bool> {
            match prop {
                $($name => Some(a.$field != b.$field),)+
                _ => None,
            }
        }

        /// Fails to compile when the struct gains a field that isn't listed as a property above
        /// (or one of the explicitly skipped ones) — a new saved field must decide how it syncs.
        #[allow(dead_code)]
        fn $guard(obj: &$ty) {
            let $ty { $($field: _,)+ $($skip: _),* } = obj;
        }
    };
}

props_table!(
    Paragraph,
    PARA_PROPS,
    para_value,
    set_para,
    para_differs,
    every_para_field_is_a_property,
    (rev),
    ((text, "text"), (runs, "runs"), (props, "props"), (mark, "mark"), (objects, "objects"), (section, "section"),)
);

props_table!(
    Table,
    TABLE_PROPS,
    table_value,
    set_table,
    table_differs,
    every_table_field_is_a_property,
    (),
    ((props, "props"), (grid, "grid"), (rows, "rows"), (controls, "controls"),)
);

props_table!(
    Part,
    STORY_PROPS,
    story_value,
    set_story,
    story_differs,
    every_part_field_is_a_property,
    (blocks),
    ((kind, "partKind"), (body, "textBody"),)
);

/// A default part kind when a `Create` arrives without a `partKind` property (the diff always
/// sends one, so this only feeds lenient applies and tests).
#[must_use]
pub fn default_part_kind() -> PartKind {
    PartKind::Header
}

/// The document-level properties (spec §2.3's whole-document scalars). The lists sync per entry
/// (see [`crate::schema`]); `media` and `passthrough` deliberately do **not** appear here: media
/// bytes ride the asset channel, never a batch (spec §3.5), and passthrough parts are
/// source-format round-trip data the local file owns.
pub const DOC_PROPS: &[&str] = &["settings", "core", "lastSection", "numbering", "sources", "customProps", "docDefaults"];

/// Write a document-level property; `None` when `prop` is not one of the synced scalars.
pub fn set_doc_prop(doc: &mut wordcraft_doc::Document, prop: &str, value: &Value) -> Option<Result<(), String>> {
    let json = value.to_json();
    match prop {
        "settings" => Some(parse(&json).map(|v| doc.settings = v)),
        "core" => Some(parse(&json).map(|v| doc.core = v)),
        "lastSection" => Some(parse(&json).map(|v| doc.last_section = v)),
        "numbering" => Some(parse(&json).map(|v| doc.numbering = v)),
        "sources" => Some(parse(&json).map(|v| doc.sources = v)),
        "customProps" => Some(parse(&json).map(|v| doc.custom_props = v)),
        "docDefaults" => {
            // StyleSheet's docDefaults ride one scalar so the styles themselves sync as entries
            // (see schema.rs) while the defaults stay document-level.
            let chr = json.get("defaultChr");
            let para = json.get("defaultPara");
            let present = json.get("defaultParaPresent").and_then(serde_json::Value::as_bool);
            if chr.is_none() && para.is_none() && present.is_none() {
                return Some(Err("docDefaults needs at least one of its three fields".into()));
            }
            let mut err = None;
            if let Some(c) = chr {
                match serde_json::from_value::<wordcraft_doc::props::CharProps>(c.clone()) {
                    Ok(v) => doc.styles.default_chr = v,
                    Err(e) => err = Some(e.to_string()),
                }
            }
            if let Some(p) = para {
                match serde_json::from_value::<wordcraft_doc::props::ParaProps>(p.clone()) {
                    Ok(v) => doc.styles.default_para = v,
                    Err(e) => err = Some(e.to_string()),
                }
            }
            if let Some(p) = present {
                doc.styles.default_para_present = p;
            }
            Some(match err {
                Some(e) => Err(e),
                None => Ok(()),
            })
        }
        _ => None,
    }
}

/// The value of a document-level property; `None` when `prop` is not a synced scalar.
pub fn doc_prop_value(doc: &wordcraft_doc::Document, prop: &str) -> Option<Value> {
    match prop {
        "settings" => Some(Value::Json(serde_json::to_value(&doc.settings).ok()?)),
        "core" => Some(Value::Json(serde_json::to_value(&doc.core).ok()?)),
        "lastSection" => Some(Value::Json(serde_json::to_value(&doc.last_section).ok()?)),
        "numbering" => Some(Value::Json(serde_json::to_value(&doc.numbering).ok()?)),
        "sources" => Some(Value::Json(serde_json::to_value(&doc.sources).ok()?)),
        "customProps" => Some(Value::Json(serde_json::to_value(&doc.custom_props).ok()?)),
        "docDefaults" => Some(Value::Json(serde_json::json!({
            "defaultChr": &doc.styles.default_chr,
            "defaultPara": &doc.styles.default_para,
            "defaultParaPresent": doc.styles.default_para_present,
        }))),
        _ => None,
    }
}

/// Deserialize a value, mapped to the adapter's error shape (wrong shapes are errors, never
/// panics).
fn parse<T: for<'de> serde::Deserialize<'de>>(json: &serde_json::Value) -> Result<T, String> {
    serde_json::from_value(json.clone()).map_err(|e| e.to_string())
}
