# Zotero

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** minor (failed commands discard their edits, #1330).

WordCraft works with the [Zotero](https://www.zotero.org) desktop app the way Word does: Zotero's
own citation dialog, all of its citation styles, automatic bibliography and refresh. Nothing to
install in Zotero; it only has to be running. The **Zotero** tab in the ribbon has the buttons:
Add/Edit Citation, Add Note, Add/Edit Bibliography, Refresh, Document Preferences and Unlink
Citations (each runs one of the commands below).

## Commands

| Command | What it does |
|---|---|
| `ui.zotero.addEditCitation` | Add a citation at the caret, or edit the one the caret is in |
| `ui.zotero.addEditBibliography` | Add the bibliography at the caret, or edit it |
| `ui.zotero.refresh` | Update every citation and the bibliography |
| `ui.zotero.setDocPrefs` | Citation style, language, footnotes or in-text |
| `ui.zotero.removeCodes` | Unlink citations: keep the text, drop Zotero's codes |
| `ui.zotero.addNote` | Insert a Zotero note |
| `ui.zotero.status` | `{busy, waitingForAnswer, last}` |
| `ui.zotero.answer` | `{"button": n}`: answer a question Zotero is asking (OK/Yes 1, No/Cancel 0; with Yes/No/Cancel: Yes 2, No 1, Cancel 0) |

They run in the app (and through the control channel and MCP bridged to the app). A command
returns at once with `{"started": …}`; Zotero then works in its own window and WordCraft answers
it in the background. Changes become visible together when the command completes successfully,
as one undo step. Cancelling, disconnecting or failing a callback discards the command's changes;
the document, selection and undo/redo history stay intact, and autosave never sees partial
citations. If you edit the document or replace it with New/Open while Zotero is working, its
result is refused with an error asking you to try again. Your edits or replacement document stay
intact. Moving the caret alone does not discard the result or move the caret back.
When Zotero asks about an existing citation, WordCraft still highlights its original position.
If the command ends unsuccessfully, that preview restores the previous selection unless you
have since moved the caret or edited the document. New citations appear only on commit.

From a terminal, against a file:

```
wordcraft-cli zotero refresh paper.docx --save paper.docx [--trace]
```

The CLI saves only after successful completion with no callback errors. An incomplete or failed
operation exits unsuccessfully and neither creates nor overwrites the `--save` destination.

Two engine commands work with Zotero's fields without Zotero running:

- `caret.pastCitation` (Zotero › Move Past Citation) moves the caret to just after the citation
  it is in or before. Adjacent citations count as one. If there is none, it moves past the next
  citation.
- Read Aloud (`review.readAloud`, Review › Speech, Mod+Alt+Space) skips citations and the
  bibliography by default. This covers Zotero, Mendeley, EndNote and Word's own citation
  fields. The player window has previous/next sentence, pause, stop and speed (0.5–3×).
  Its commands are `readAloud.playPause`, `.next`, `.previous`, `.stop`, `.speed {"value"}`,
  `.skipCitations {"value"}` and `.status`. Set `WORDCRAFT_SPEECH=off` to keep it silent.

Switching between an in-text style and a note style in Document Preferences moves the citations:
into new footnotes (or endnotes) at the same place, and back into the text where the note marks
were; a note that held only the citation is removed.

## Word compatibility

Citations are stored as Word's Zotero plugin stores them — fields with the code
`ADDIN ZOTERO_ITEM CSL_CITATION {…}`, the bibliography as `ADDIN ZOTERO_BIBL … CSL_BIBLIOGRAPHY`,
document preferences in the custom properties `ZOTERO_PREF_1…n` — so one document can be edited
in Word and WordCraft in turn, with Zotero working in both. (Zotero talks to WordCraft over the
protocol it uses for LibreOffice and calls the field type "Reference Mark" there; WordCraft keeps
Word's "Field" in the file.)

## Limits

- Zotero's "Bookmarks" field type isn't supported; keep the default.
- Zotero's document transfer (Export/Import Document) isn't needed and isn't supported.
- The web version can't reach Zotero (browsers can't open the socket).

## How it works

`crates/zotero` implements Zotero's published word-processor wire protocol (port 23116):
`client` runs a session, `Bridge` answers Zotero's `Document_*` / `Field_*` calls against a private
editing session owned by `Transaction`. Each failed callback restores its own edits and bridge
state; the transaction commits only after a successful completion reply and a check that the live
document has not changed. `client::run_on_session` uses this transaction automatically; callers
of the lower-level `run_command`/`serve` must check the returned `Outcome` themselves. The app
(`crates/ui-egui/src/zotero.rs`) runs the socket on a thread and answers on the UI thread.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-15 | minor | Atomic commands, conflict handling and CLI save gating (#1330); synthetic protocol and app tests, not a live Zotero interoperability check |
