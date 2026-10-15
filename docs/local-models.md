# Dictate and Translate with local models

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** major (first version)

Dictate (Home › Voice) and Translate Document (Review › Proofing, and Review › Language ›
Translate) run on your own computer, with model files you download once. Nothing you say or write
is sent anywhere, and WordCraft never downloads anything by itself: you choose the files.

| | Dictate | Translate |
|---|---|---|
| Model | OpenAI [Whisper](https://github.com/openai/whisper), speech to text in ~99 languages | [Marian](https://marian-nmt.github.io/) translation models, such as Helsinki-NLP's [OPUS-MT](https://github.com/Helsinki-NLP/Opus-MT) — one model per language pair |
| Licence of the weights | MIT (OpenAI's release; the Hugging Face copies are marked Apache-2.0) | CC-BY 4.0 (OPUS-MT; some Hugging Face copies are marked Apache-2.0) — credit "Helsinki-NLP / OPUS-MT" if you redistribute them |
| Files in the folder | `config.json`, `tokenizer.json`, `model.safetensors` | `config.json`, `source.spm`, `target.spm`, `vocab.json`, `model.safetensors` |
| Setting | File › Options › Language › Speech model folder | File › Options › Language › Translation models folder |
| Runs on | the CPU, on a worker thread | the CPU, on a worker thread |

The models run on [candle](https://github.com/huggingface/candle) (pure Rust, MIT/Apache-2.0).
The web version of WordCraft has no models; Dictate and Translate say so there.

## Getting a speech model (Whisper)

Download these three files from a Whisper model on Hugging Face into one folder of your own
(for example `Documents/WordCraft models/whisper-base`):

| Model | Size | Speed on a laptop CPU | Quality |
|---|---|---|---|
| [openai/whisper-tiny](https://huggingface.co/openai/whisper-tiny/tree/main) | 150 MB | ~1 s per phrase | fine for clear English; weak in other languages |
| [openai/whisper-base](https://huggingface.co/openai/whisper-base/tree/main) | 290 MB | ~2 s | a good start |
| [openai/whisper-small](https://huggingface.co/openai/whisper-small/tree/main) | 970 MB | ~5 s | much better for Hebrew, Arabic and other languages |
| [openai/whisper-medium](https://huggingface.co/openai/whisper-medium/tree/main) | 3 GB | slow | best that fits most machines |

Files: `config.json`, `tokenizer.json`, `model.safetensors`. The `.en` models (`whisper-base.en`…)
hear only English and are a little better at it. Large models (`whisper-large-v3`) work too, but
need a fast computer and ~6 GB of memory.

Then File › Options › Language › **Speech model folder** › Browse… and pick any of the three files
(or type the folder and press Apply). **Dictation language** says what you will speak; *Detect
automatically* lets the model decide for each phrase (short phrases are sometimes mistaken for
another language: pick yours if that happens).

## Getting translation models (OPUS-MT)

Make one folder for them (for example `Documents/WordCraft models/translation`) and put each
model in a subfolder named after the model, such as `opus-mt-en-he`: the name says the language
pair (English → Hebrew). For each model, download from its Hugging Face page:

- `config.json`, `source.spm`, `target.spm`, `vocab.json`, and
- `model.safetensors`. Many OPUS-MT pages only have `pytorch_model.bin` on their main branch, in
  an old PyTorch format WordCraft can't read; their `model.safetensors` is on the page's
  "Files and versions" › branch list, in the `refs/pr/…` branch titled *Adding safetensors
  variant of this model* (for example
  [opus-mt-en-he](https://huggingface.co/Helsinki-NLP/opus-mt-en-he/tree/refs%2Fpr%2F2)).

Some pairs: [en-he](https://huggingface.co/Helsinki-NLP/opus-mt-en-he),
[tc-big-he-en](https://huggingface.co/Helsinki-NLP/opus-mt-tc-big-he-en) (Hebrew → English; has
`model.safetensors` on main), [en-de](https://huggingface.co/Helsinki-NLP/opus-mt-en-de),
[de-en](https://huggingface.co/Helsinki-NLP/opus-mt-de-en),
[en-fr](https://huggingface.co/Helsinki-NLP/opus-mt-en-fr),
[en-es](https://huggingface.co/Helsinki-NLP/opus-mt-en-es),
[en-ru](https://huggingface.co/Helsinki-NLP/opus-mt-en-ru),
[en-ar](https://huggingface.co/Helsinki-NLP/opus-mt-en-ar),
[en-zh](https://huggingface.co/Helsinki-NLP/opus-mt-en-zh). Models into several languages
(`opus-mt-en-mul`, `opus-mt-en-ROMANCE`) work too: WordCraft adds the target-language token.
Models with separate source and target vocabularies aren't supported yet. Do **not** use NLLB or
other models whose licence forbids commercial use.

Then File › Options › Language › **Translation models folder** › Browse… and pick a file in any
model folder: WordCraft keeps the folder above it, so every pair beside it is found. The line
under the setting lists the pairs it found.

## Using them

- **Dictate:** Home › Dictate. The Dictation window says when the model has loaded; speak, and
  each phrase is written at the cursor when you pause (a pause of about 0.7 s ends a phrase; one
  phrase is at most 25 seconds). Each phrase is one Undo step. Press Dictate again or Stop to
  finish; the last phrase is still written. Change the language in the Dictation window.
- **Translate Document:** Review › Translate Document (or Review › Translate). Choose the target
  language and, if you like, the source (otherwise guessed from the text's script and the
  document's language). The whole document goes into a new, untitled document by default — the
  original file is never touched, and Undo brings the original text back — or into this document;
  with a selection, the selected paragraphs are translated in place. A progress window shows how
  far it is and can cancel.

What Translate keeps: paragraphs and their styles, lists and numbering, tables, pictures, shapes,
text boxes, headers and footers, footnotes, comments, fields, bookmarks, sections and page
setup. Character formatting is mapped onto the translation: the formatting most of a stretch has
becomes the translation's, and each differently formatted word or phrase (bold, a link, a colour)
is translated on its own and found in the translation; where the model words it differently, the
formatting goes to the words at the same place. Into Hebrew, Arabic and Persian, paragraphs
become right-to-left and the complex-script font and size follow the text's. Citations made by
Zotero, Mendeley or EndNote and tracked deletions aren't translated.

## For scripts and agents

Both are commands (`wordcraft-cli commands`), so they work headlessly too:

```sh
wordcraft-cli run --file letter.docx \
  --cmd 'file.options={"translationModels": "/models/translation"}' \
  --cmd 'review.translate={"to": "he"}' --save letter-he.docx
wordcraft-cli run --cmd 'tools.dictate={"path": "memo.wav", "model": "/models/whisper-base"}' --save memo.docx
```

`review.translate` also takes `translations` (a map of text → translation) instead of a model, so
an agent can translate with anything and still keep the formatting; `tools.dictate` takes `text`.
Under `--automation-read-root`, model folders and audio files must be inside the root.

## Known limits

- The first phrase or translation waits for the model to load (a second for tiny models, longer
  for large ones).
- Dictate writes whole phrases, not words as you speak; spoken punctuation ("comma", "new line")
  isn't turned into marks — Whisper punctuates by itself.
- SentencePiece's NFKC normalisation table isn't applied, so compatibility characters (full-width
  Latin letters, ligatures) may translate less well.
- Translate Selection translates the selected paragraphs whole.
- Into a new document, the window's document becomes the untitled translation (WordCraft has one
  document per window); Undo brings the original text back, but as an untitled document: save it
  with Save As, or reopen the original file, which Translate never changes.
- Charts, SmartArt and WordArt text aren't translated yet.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-15 | major | First version: Dictate with Whisper, Translate Document with Marian/OPUS-MT, both local |
