# Greek localization and spelling (`el`, `el-GR`)

WordCraft supports a Greek user interface and Modern Greek spelling checks. Choose **Ελληνικά** in **File → Options → Language**, or let the app follow the operating system language. New documents created under the Greek interface use `el-GR` as their editing/proofing language. Existing documents retain their language tags, and can use mixed-language runs.

The interface catalog is `crates/ui-egui/src/i18n/el.tsv`. It preserves source placeholders, Unicode ellipses and context fields. Technical names (WordCraft, Markdown, Zotero, etc.) and format codes are deliberately unchanged.

## Installing the spelling dictionary

WordCraft **does not bundle** the dictionary. Install a compatible Hunspell `el_GR.aff` and `el_GR.dic` pair in `~/Library/Spelling` on macOS, `/usr/share/hunspell` on Linux, or choose a folder under **File → Options → Proofing → Dictionaries**. The `el_GR` dictionary published by the LibreOffice dictionaries project uses **ISO-8859-7**, which WordCraft can decode directly. Different dictionaries may have different licenses; use only where permitted and do not add third-party dictionaries to this repository.

Modern Greek spelling is checked **offline**, against the installed dictionary. It supports red underlines and correction suggestions for tonos, word-final sigma, normal typos, and simple Hunspell `REP` replacements. Without an installed dictionary, Greek words are not flagged and a missing-dictionary hint is displayed. Polytonic/Ancient Greek is **deliberately not** assessed by the Modern Greek dictionary; grammatical analysis and Greek hyphenation are not claimed.

## Tests

Run after local build dependencies are ready (not required just to update the translation):

```sh
cargo test -p wordcraft-proof --lib
cargo test -p wordcraft-ui-egui --lib i18n::tests
```

For the full application, run `cargo run -p wordcraft --release` only when ready to verify the desktop UI; this is a separate, potentially long compile. Before submitting the PR run `git diff --check` and `cargo fmt --all -- --check`.
