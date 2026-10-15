# English spelling foundation

English spelling uses explicit full forms from the English Speller Database (ESDB), plus
WordCraft's explicit supplement and user/ignore dictionaries. The Moby dictionary continues
to provide hyphenation; its entries no longer license spelling derivations.

A known stem is insufficient evidence for arbitrary affixes: `buy` must not license `buyed`,
nor `cat` license `catest`. Inflected forms such as `bought`, `running` and `stopped` are listed
independently. Unknown forms are flagged without attempting unrestricted stem stripping.
User additions are accepted exactly and do not license derivations either.

The lexicon is embedded UTF-8, validated at compile time. A single immutable, lazily initialized
index borrows its word strings and provides binary-search membership and character-length
buckets for suggestions. Case handling permits initial capitals and uppercase text without
lowercasing proper nouns into ordinary dictionary words. Apostrophe normalization stays at
the English API boundary. Existing ignore settings and per-run language dispatch remain.

Suggestions draw only from the spelling lexicon and explicit supplement, excluding ESDB's
NOSUGGEST entries. Candidate lengths differ by at most two characters. Bounded optimal string
alignment scoring reuses fixed scratch rows across candidates. Ranking remains edit distance,
proper-name penalty for lowercase input, first letter, length difference and deterministic
lexical order; it is not frequency ranking.

## Deliberate trade-offs

Explicit recognition removes the blanket affix rules' false-acceptance mechanism, but gives
up their open-ended coverage. A legitimate rare word, recent coinage, specialist term or
inflection missing from the selected data may now be flagged. An unlisted form is not proof
of linguistic invalidity. During implementation the existing `uploader` regression exposed
this coverage cost; `uploader` and `uploaders` were added explicitly to the supplement.
Further gaps should be resolved with source-backed data updates, reviewable supplement
entries or user additions, rather than restoring unrestricted derivation.

Full forms make acceptance reproducible and independently inspectable, at the cost of bundled
data and an in-memory index. The generated text asset is 1,089,739 bytes before the supplement;
resident-memory and startup costs have not been measured. No new dependency or external
service is required to use the checker, but regenerating its data requires the documented
upstream tools. Lexicon versioning and coverage maintenance become explicit responsibilities.

The regional union accepts American, British and Canadian variants rather than enforcing a
document's particular English dialect. Proper-name casing is now stricter: a listed `London`
does not automatically license lowercase `london`. Suggestion membership is consistent with
the spelling lexicon, but edit-distance ranking does not establish that its first suggestion
is the intended correction. Neither vocabulary size nor these targeted regressions establish
corpus-wide accuracy or superiority over other spelling engines.

## Data and reproducible generation

Source: https://github.com/en-wl/wordlist at
`1e5b7d3a72f47a71da5d28686c1dd4b397178485`.

The extraction uses size 60, American/British/Canadian spellings `A,B,Z,C`, variant level 1,
hyphenated forms, and source NOSUGGEST annotations. This intentionally preserves the existing
acceptance of multiple English regions; dialect-specific enforcement is future work. Size 60
is the upstream recommended general spellchecking size. This is a choice of lexical coverage,
not a claim that every spelling decision or every English genre is correct.

With Python 3 and Git available, create the upstream database using its unmodified tooling:

```text
git clone https://github.com/en-wl/wordlist esdb
git -C esdb checkout 1e5b7d3a72f47a71da5d28686c1dd4b397178485
cd esdb
python -X utf8 combine.py create-db scowl.db
cd ../wordcraft
cargo run -p wordcraft-proof --example build_en -- ../esdb <python-executable> assets/spelling/en-forms.txt
```

Use an absolute Python executable path if it is not on PATH. UTF-8 mode is required on Windows.
The Rust converter verifies the source revision and refuses modified tracked source, merges
duplicate records conservatively, sorts by UTF-8 word bytes and writes LF line endings.
Its input is the upstream generated database; recreate it with the command above before export.
No generator, Python installation or network is required by WordCraft at runtime or build time.

The generated file contains 114,616 forms. Its SHA-256 is recorded in ATTRIBUTION.md.
Full upstream permission notices are preserved in `assets/spelling/LICENSE-ESDB.txt` and NOTICE.
The supplement contains explicit forms; update it deliberately rather than reintroducing
blanket inflection rules.

## Validation and boundaries

Regression tests check invalid regularizations, retained irregular/regular forms, source
ordering, suggestion eligibility and case behavior. Property tests compare scratch-row reuse
with the existing distance implementation, including Unicode and successive candidate lengths.
The existing proofing suite exercises the public English API and mixed-language integration.

This change does not implement contextual grammar, productive morphological analysis,
frequency ranking, dialect selection, or the other shared-proofing fixes. It deliberately
replaces the English lexical decision model without extending the language-independent
Hunspell parser. General accuracy requires an independently labeled corpus beyond these
regressions.
