//! The adapter's convergence suite (CraftHub spec §3.6 phase B: the ported tests, the family's
//! most valuable testing asset), over WordCraft's real document: the roundtrip guarantee with
//! the RGA text discipline, block identity across edits, and the two-client convergence of
//! concurrent paragraph writing — the `textcrdt` row's whole point.

// Tests may unwrap: a failing assertion is the point of a test (the family's clippy rule).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_collab::adapter::HubAdapter;
use wordcraft_doc::{Block, CharProps, Document, Paragraph, Part, PartKind, StoryRef, Table};
use wordcraft_hub::WordcraftHub;

fn para(text: &str) -> Block {
    Block::Para(Paragraph::with_text(text, CharProps::default()))
}

fn sample() -> Document {
    let mut doc = Document {
        body: vec![
            std::sync::Arc::new(para("The first draft opens with a claim.")),
            std::sync::Arc::new(para("The second paragraph carries the evidence.")),
            std::sync::Arc::new(Block::Table(Table::default())),
            std::sync::Arc::new(para("The last paragraph concludes.")),
        ],
        ..Default::default()
    };
    // A header part with one paragraph.
    doc.parts.insert(0, Part { kind: PartKind::Header, blocks: vec![std::sync::Arc::new(para("Running head"))], ..Default::default() });
    doc
}

fn assert_same_state(hub: &WordcraftHub, a: &Document, b: &Document, what: &str) {
    let (va, vb) = (hub.view(a), hub.view(b));
    assert_eq!(va.nodes.len(), vb.nodes.len(), "{what}: node count");
    for (na, nb) in va.nodes.iter().zip(vb.nodes.iter()) {
        assert_eq!(na.id, nb.id, "{what}: node id");
        assert_eq!(na.kind, nb.kind, "{what}: kind of {na:?}");
        assert_eq!(na.parent, nb.parent, "{what}: parent of {na:?}");
        assert_eq!(na.position, nb.position, "{what}: position of {na:?}");
        assert_eq!(
            na.props,
            nb.props,
            "{what}: props of {na:?} (text: {:?} vs {:?})",
            na.props.iter().find(|(k, _)| k.as_ref() == "text").map(|(_, v)| format!("{v:?}")),
            nb.props.iter().find(|(k, _)| k.as_ref() == "text").map(|(_, v)| format!("{v:?}"))
        );
    }
    assert_eq!(va.entries, vb.entries, "{what}: entries");
    assert_eq!(va.doc, vb.doc, "{what}: doc props");
}

/// The core promise, kept: applying the diff of two documents to the first makes it the second.
#[test]
fn apply_diff_roundtrip() {
    let hub = WordcraftHub::new();
    let a = sample();
    let mut b = a.clone();

    // A text edit inside a paragraph (the RGA path). The app's edit commands keep the runs
    // coherent with the text; the fixture does the same.
    {
        let p = b.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(1)).unwrap();
        p.text = "The second paragraph carries the whole evidence.".into();
        p.runs = vec![wordcraft_doc::Run { len: p.text.len(), props: wordcraft_doc::CharProps::default() }];
    }

    // Formatting props edit on another paragraph.
    b.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap().props.style = Some("Heading1".into());

    // Insert a block in the middle, delete one, move the last up.
    b.body.insert(2, std::sync::Arc::new(para("An inserted aside.")));
    b.body.remove(3); // the table
    let last = b.body.remove(3);
    b.body.insert(0, last);

    // The header part's text changes (a story property set).
    b.parts.get_mut(&0).unwrap().blocks[0] = std::sync::Arc::new(para("Running head, revised"));

    // Document level.
    b.core.title = "The edited title".into();

    let changes = hub.diff(&a, &b);
    assert!(!changes.is_empty());

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "roundtrip");
    // The roundtrip is stable: diffing again finds nothing.
    assert!(hub.diff(&applied, &b).is_empty(), "the applied state diffs clean against the target");
    // And the visible text landed byte for byte.
    assert_eq!(applied.plain_text(StoryRef::Body), b.plain_text(StoryRef::Body));
}

/// Two clients writing inside the SAME paragraph both survive — the textcrdt row's whole point
/// (spec §6.6): character-level merge instead of whole-string last-writer-wins.
#[test]
fn concurrent_paragraph_writing_merges() {
    // Two adapters, one per client (the real shape: a session owns one adapter). Both assign
    // the same ids to the shared base at first sync (the deterministic space-0 walk), so the
    // deltas cross-apply.
    let hub_l = WordcraftHub::with_space(1);
    let hub_r = WordcraftHub::with_space(2);
    let base = sample();

    let mut local = base.clone();
    let mut remote = base.clone();
    // Client 1 extends the end; client 2 inserts in the middle. Both against the same base.
    {
        let p = local.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
        p.text = "The first draft opens with a claim, stated plainly.".into();
        p.runs = vec![wordcraft_doc::Run { len: p.text.len(), props: wordcraft_doc::CharProps::default() }];
    }
    {
        let p = remote.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
        p.text = "The very first draft opens with a claim.".into();
        p.runs = vec![wordcraft_doc::Run { len: p.text.len(), props: wordcraft_doc::CharProps::default() }];
    }

    let local_batch = hub_l.diff(&base, &local);
    let remote_batch = hub_r.diff(&base, &remote);

    // Both sides receive both batches (the server echoes each to everyone).
    hub_l.apply(&mut local, &remote_batch);
    hub_r.apply(&mut remote, &local_batch);

    // Both sentences' edits survive — neither author's words are lost.
    let la = local.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap().text.clone();
    let ra = remote.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap().text.clone();
    assert_eq!(la, ra, "the two clients converged");
    assert!(la.contains("very first"), "the remote's insertion survived: {la}");
    assert!(la.contains("stated plainly"), "the local's extension survived: {la}");
    // Convergence as the rendered documents (the honest cross-client check: each adapter views
    // its own document, so the ids are per-hub — the text is the shared truth).
    assert_eq!(local.plain_text(StoryRef::Body), remote.plain_text(StoryRef::Body));
    assert_eq!(local.styles, remote.styles);
    assert_eq!(local.parts, remote.parts);
}

/// Two clients editing DIFFERENT paragraphs converge trivially, and neither touches the other.
#[test]
fn two_clients_converge_on_different_paragraphs() {
    let hub_l = WordcraftHub::with_space(1);
    let hub_r = WordcraftHub::with_space(2);
    let base = sample();

    let mut local = base.clone();
    let mut remote = base.clone();
    {
        let p = local.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap();
        p.text = "Local rewrite of the opening.".into();
        p.runs = vec![wordcraft_doc::Run { len: p.text.len(), props: wordcraft_doc::CharProps::default() }];
    }
    {
        let p = remote.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(3)).unwrap();
        p.text = "Remote rewrite of the conclusion.".into();
        p.runs = vec![wordcraft_doc::Run { len: p.text.len(), props: wordcraft_doc::CharProps::default() }];
    }

    let local_batch = hub_l.diff(&base, &local);
    let remote_batch = hub_r.diff(&base, &remote);
    hub_l.apply(&mut local, &remote_batch);
    hub_r.apply(&mut remote, &local_batch);
    // Convergence as the rendered documents (see the same-paragraph test's note).
    assert_eq!(local.plain_text(StoryRef::Body), remote.plain_text(StoryRef::Body));
    assert_eq!(local.styles, remote.styles);
    assert_eq!(local.parts, remote.parts);
    assert!(local.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(0)).unwrap().text.contains("Local"));
    assert!(local.para_mut(StoryRef::Body, &wordcraft_doc::Path::top(3)).unwrap().text.contains("Remote"));
}

/// An insertion in the middle costs one position key — the untouched blocks keep theirs.
#[test]
fn insertion_in_the_middle_costs_one_key() {
    let hub = WordcraftHub::new();
    let a = sample();
    let mut b = a.clone();
    b.body.insert(1, std::sync::Arc::new(para("Inserted second.")));

    let changes = hub.diff(&a, &b);
    let positions = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Set { prop, .. } if prop.as_ref() == "position")).count();
    let creates = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Create { .. })).count();
    assert_eq!(creates, 1, "one created block");
    assert_eq!(positions, 1, "exactly one position write: the inserted block");

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "insert");
    // And the inserted block really sits second.
    let second = match &*applied.body[1] {
        Block::Para(p) => p.text.clone(),
        Block::Table(_) => String::new(),
    };
    assert!(second.contains("Inserted second"));
}

/// The style sheet syncs per entry (the document-level keyed list).
#[test]
fn style_sheet_syncs_per_entry() {
    let hub = WordcraftHub::new();
    let a = sample();
    let mut b = a.clone();
    b.styles.styles.push(wordcraft_doc::Style {
        id: "MyStyle".into(),
        name: "My Style".into(),
        kind: wordcraft_doc::StyleKind::Paragraph,
        based_on: None,
        next: None,
        linked: None,
        para: wordcraft_doc::ParaProps::default(),
        chr: wordcraft_doc::CharProps::default(),
        priority: None,
        quick: false,
        hidden: false,
        builtin: false,
        locked: false,
        default: false,
        table: None,
    });

    let changes = hub.diff(&a, &b);
    let puts = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Put { .. })).count();
    assert_eq!(puts, 1, "one Put for the new style");

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "styles");
    assert_eq!(applied.styles.styles.len(), b.styles.styles.len());
    assert_eq!(applied.styles.styles.last().unwrap().id, "MyStyle");
}

/// A deleted block is one change, and deleting a deleted block does nothing.
#[test]
fn delete_is_one_change_and_tolerant() {
    let hub = WordcraftHub::new();
    let a = sample();
    let mut b = a.clone();
    b.body.remove(0);

    let changes = hub.diff(&a, &b);
    let deletes = changes.iter().filter(|c| matches!(c, craft_collab::change::Change::Delete { .. })).count();
    assert_eq!(deletes, 1, "one Delete for the removed block");

    let mut applied = a.clone();
    hub.apply(&mut applied, &changes);
    assert_same_state(&hub, &applied, &b, "delete");
    // Tolerant re-apply: deleting a deleted block does nothing.
    let report = hub.apply(&mut applied, &changes);
    assert_eq!(report.applied, 0);
}

/// The command catalog resolves the v2 names onto their `hub.*` twins (§18.10 compatibility),
/// and the deep link parses the family format.
#[test]
fn catalog_and_deeplink() {
    assert_eq!(wordcraft_hub::commands::resolve("collab.share").map(|c| c.name), Some("hub.share"));
    assert_eq!(wordcraft_hub::commands::resolve("hub.peers").map(|c| c.name), Some("hub.peers"));
    let link = wordcraft_hub::deeplink::JoinLink::parse("wordcraft://join?code=AB12CD34&server=wss%3A%2F%2Fhub.example.com");
    assert_eq!(link.map(|l| (l.code, l.server)), Some((Some("AB12CD34".into()), Some("wss://hub.example.com".into()))));
}
