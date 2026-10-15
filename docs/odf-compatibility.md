# Writer document compatibility

Status: 2026-10-14. ODT, OTT and FODT share the same reader and writer. These are
editable imports, with a bounded subset of ODF; they are not lossless package editors.

The shared path preserves body paragraphs, tables, pictures, explicit paragraph spacing,
page size and margins, master-page headers and footers, footnotes/endnotes (including custom
marks), comment ranges with author/date, and tracked text insertions/deletions. Section imports
read paragraph/table master-page switches, page-number restarts and continuous column sections.
Exports retain page-style switches and continuous equal-width column sections. Header/footer
stories use the same style and media conversion as body text, including inline FODT pictures.
First-page and even-page stories are conditional on the corresponding document settings.

Use the existing open/save/export commands or CLI conversion. OTT packages have the template
media type; FODT is a standalone XML document with inline binary pictures. No external image
or XML references are fetched. Imported limits include 10,000 sections, 10,000 changed regions,
64 simultaneously open insertions, 10,000 notes/comments and 16 MiB of collected note/comment
events. Master-page stories have a separate 16 MiB collection budget.

## Remaining limitations

- Text boxes, arbitrary drawing objects, live fields, note numbering configuration and
  unequal-width columns are not fully represented. A displayed page-number field is plain text.
- Nested notes/comments and notes/comments in header/footer stories are not imported.
- Comment replies/resolution and format-only revisions are not preserved; tracked deleted
  pictures/tables and revisions in auxiliary stories are not complete.
- Continuous sections that change page geometry or headers are not faithfully exportable;
  their supported export is a column change on the current page style. Odd/even page-start
  semantics, complex floating layouts and exact pagination still need work.
- Style defaults, inherited document formatting and unsupported ODF extensions are not a
  promise of LibreOffice-equivalent layout. Template instantiation semantics are unchanged.

OTT/FODT exports remain lossy copies and do not clear the source document's dirty state.
The broader per-document loss-report/save-policy work is tracked separately in PR #1511.

## Integration provenance

This work integrates the notes/comments implementation from [#1057](https://github.com/storytold/wordcraft/pull/1057),
the section reader from [#1506](https://github.com/storytold/wordcraft/pull/1506), and RTF/ODT
revision work from [#1516](https://github.com/storytold/wordcraft/pull/1516) and
[#1517](https://github.com/storytold/wordcraft/pull/1517), on the OTT/FODT path from
[#1639](https://github.com/storytold/wordcraft/pull/1639). Header/footer integration also considers
the master-page behavior covered by [#873](https://github.com/storytold/wordcraft/pull/873).
Those contributions are credited to their original authors; integration adds shared flat/package
handling, section export, master-page story export and combined regression coverage.

Revision history: 2026-10-14 — document the combined ODF paths and remaining fidelity limits.
