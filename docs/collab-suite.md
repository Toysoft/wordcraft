# The collaboration behavior suite (WordCraft's projections)

> The shared suite every Craft app runs before its CraftHub integration is enabled (the CraftHub
> specification §18.9): each row is one automated test with a written scenario, in the family's
> behaviour-suite tradition. This file is WordCraft's copy of the shared catalog: the scenarios
> are identical in every app, and the *projections* column is where each app writes what the
> scenario means for its own UI.

## The scenarios

| # | Scenario | The guarantee under test | WordCraft's projection |
|---|----------|--------------------------|------------------------|
| 1 | Join by code, join by link | One argument that recognizes itself; the file opens in its own tab, other sessions untouched | A joined document opens as its own tab; the status bar and margin stay per-document |
| 2 | Concurrent editing converges | Snapshot-hash equality after arbitrary interleavings, drops and reconnects (per seed) | **Character-level inside a paragraph** — the `textcrdt` row: both authors' sentences survive, runs repaired to cover the merged text |
| 3 | Read-only, with its reason | The reason is shown (view link, role without edit, region lock), never just a badge | The mode bar shows "view link" / "viewer role" / "locked region" with the locked range tinted in the margin |
| 4 | Batch refused with fixes | The server's refusal carries the fixes; the client applies them and lands on the server state | A refused paragraph structure snaps back with a toast explaining why |
| 5 | Per-user undo | My steps are mine alone; later writes by others survive my undo | Undo after a peer's edit undoes only my edit; the peer's sentence stays word for word |
| 6 | Offline, then resume as the same client | The recovery copy reattaches; unsent changes replay or land as a "(recovered copy)" beside the file | The unsent paragraph replays on reconnect; a conflicting copy appears in Home ▸ Online |
| 7 | Cursor chat and its bounds | ≤100 characters, ephemeral, never enters the document or the journal | The bubble renders beside the peer's caret badge for its lifetime, then is gone |
| 8 | Spotlight and follow | Follow tracks view and page; any view movement of mine stops the follow | The scroll animates to the presenter's page and position; my own scroll breaks the leash |
| 9 | Presence and per-app projections | Ephemeral, throttled to change-only, bounded sizes | The caret in a paragraph (the `editing_text` triple) is the live signal: a badge in that paragraph's margin, the page number in the bar |
| 10 | Region locks | Locks sync as document properties; locked regions tint; the reason is shown | A locked paragraph range tints; editing inside it is refused with "locked by <name>" |
| 11 | Contribution, review, merge | The branch lives inside the live session; merge is one undo step; each side's changes show | The review pane shows both sides' property diffs, Inspect-style, per paragraph |
| 12 | Comments and their anchors | Anchor kinds per app; comments are entries with ids and order | **Anchors are text ranges** (margin highlights), possibly spanning paragraphs — `WordcraftHub::comment_anchor` derives the range from the `CommentStart`/`CommentEnd` markers |
| 13 | Background file notifications | Delivered when the tab is not front; bounded, dismissible | The tab badge shows "3 new" for a backgrounded shared document |
| 14 | Closing a file ends its session alone | Other sessions keep syncing behind the front tab | Closing one shared tab never drops another document's peers |
| 15 | The revert guard | `file.revert` is refused while its document is in a live session (including reconnect and pre-welcome) | The toast says "leave the session before reverting"; the document and its previews are intact |

## WordCraft's adapter surface (what the suite drives)

* **Property tables** — three small tables with the compile-time guard: [`Paragraph`]'s fields
  (`text` opting into RGA, `runs`, `props`, `mark`, `objects`, `section`; `rev` never syncs —
  it is a process-local cache counter), [`Table`]'s fields, and the part properties of
  secondary stories (`partKind`, `textBody`).
* **Block identity** — WordCraft's blocks have no persistent ids, so the adapter owns the
  identity: a deterministic space-0 walk at first sync (two clients opening the same file
  compute the same ids — including every paragraph's initial RGA element ids), `Arc`-pointer
  tracking across clones (edits copy only what they touch), positional pairing between
  unchanged anchors for edited-in-place blocks, and fresh ids in the local client's space for
  genuinely new blocks.
* **The RGA text discipline** — one `TextDoc` per paragraph beside the document; a local edit
  composes into an edit-sized `Value::Text` delta (the inserted and tombstoned elements,
  anchored) that rides the ordinary batch; a remote delta merges element-wise and idempotently,
  the paragraph is rewritten from the merged render, and the runs are repaired to cover it.
* **The document level** — the style sheet syncs per entry with its order; the comments map
  per entry; `media` bytes never ride a batch (the asset channel owns them) and `passthrough`
  parts stay local (source-format round-trip data).
* **Commands** — the unified `hub.*` catalog with the v2 `collab.*` twins (`wordcraft-hub::commands`):
  same names in every Craft app, the explicit lexical-unification rule.
* **Deep links** — `wordcraft://join?<code\|token>&server=<url>` (`wordcraft-hub::deeplink`).

## The enable rule

No app is enabled before its protocol core passes its simulation tests in CI for a full week
(nightly, fixed seeds) — and enabling happens through the app's own `crafthub` feature flag,
never through the core, so a regression is one flag flip away from off (§18.4).
