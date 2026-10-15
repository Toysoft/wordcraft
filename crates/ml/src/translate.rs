//! Translation with a Marian model (Helsinki-NLP's OPUS-MT and others in the Hugging Face
//! layout).
// The web build has no models: the helpers here serve only the desktop translator (and tests).
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::{Error, Result, spm};

/// Longest input given to the model at once, in tokens (the models take 512 positions).
const MAX_INPUT_TOKENS: usize = 400;
/// Characters per part when a sentence is too long for the model.
const LONG_SENTENCE_CHARS: usize = 600;

/// A Marian vocabulary (`vocab.json`): piece ↔ id, shared by source and target.
#[derive(Clone, Debug)]
pub(crate) struct Vocab {
    ids: HashMap<String, u32>,
    pieces: Vec<String>,
}

impl Vocab {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let v: HashMap<String, u64> = serde_json::from_str(text).map_err(|e| Error::Model(format!("vocab.json: {e}")))?;
        let len = v.values().map(|id| *id as usize).max().map_or(0, |m| m.saturating_add(1));
        if v.is_empty() || len > 1 << 20 {
            return Err(Error::Model("vocab.json has no usable tokens".into()));
        }
        let mut pieces = vec![String::new(); len];
        let mut ids = HashMap::with_capacity(v.len());
        for (piece, id) in v {
            if let Some(slot) = pieces.get_mut(id as usize) {
                slot.clone_from(&piece);
            }
            ids.insert(piece, id as u32);
        }
        Ok(Vocab { ids, pieces })
    }

    pub(crate) fn id(&self, piece: &str) -> Option<u32> {
        self.ids.get(piece).copied()
    }

    pub(crate) fn piece(&self, id: u32) -> Option<&str> {
        self.pieces.get(id as usize).map(String::as_str)
    }
}

/// The token ids the model reads for `text`: its SentencePiece pieces (unknown ones as `<unk>`),
/// after a `>>xx<<` target-language token for models into several languages, ending with `</s>`.
pub(crate) fn encode(sp: &spm::Model, vocab: &Vocab, text: &str, target_token: Option<u32>, eos: u32, unk: u32) -> Vec<u32> {
    let mut ids: Vec<u32> = target_token.into_iter().collect();
    ids.extend(sp.encode(text).iter().map(|p| vocab.id(p).unwrap_or(unk)));
    ids.truncate(MAX_INPUT_TOKENS);
    ids.push(eos);
    ids
}

/// The text of the model's output ids, special tokens left out.
pub(crate) fn decode(vocab: &Vocab, ids: &[u32], special: &[u32]) -> String {
    let pieces: Vec<&str> = ids.iter().filter(|id| !special.contains(id)).filter_map(|id| vocab.piece(*id)).collect();
    spm::decode(&pieces)
}

/// The settings of a Marian `config.json`, with the defaults older files leave out filled in.
pub(crate) fn config_json(text: &str) -> Result<Value> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| Error::Model(format!("config.json: {e}")))?;
    let o = v.as_object_mut().ok_or_else(|| Error::Model("config.json isn't an object".into()))?;
    if o.get("model_type").and_then(Value::as_str).is_some_and(|t| t != "marian") {
        return Err(Error::Model("this isn't a Marian translation model (config.json model_type)".into()));
    }
    let get = |o: &serde_json::Map<String, Value>, k: &str| o.get(k).cloned();
    let vocab = get(o, "vocab_size").ok_or_else(|| Error::Model("config.json has no vocab_size".into()))?;
    let eos = get(o, "eos_token_id").unwrap_or(Value::from(0));
    for (k, d) in [
        ("decoder_vocab_size", vocab.clone()),
        ("forced_eos_token_id", eos.clone()),
        ("eos_token_id", eos),
        ("share_encoder_decoder_embeddings", Value::Bool(true)),
        ("activation_function", Value::from("swish")),
        ("use_cache", Value::Bool(true)),
        ("is_encoder_decoder", Value::Bool(true)),
        ("scale_embedding", Value::Bool(true)),
        ("max_position_embeddings", Value::from(512)),
    ] {
        if o.get(k).is_none_or(Value::is_null) {
            o.insert(k.to_string(), d);
        }
    }
    if o.get("share_encoder_decoder_embeddings") == Some(&Value::Bool(false)) {
        return Err(Error::Model("this model has separate source and target vocabularies, which WordCraft can't run yet".into()));
    }
    Ok(v)
}

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::Translator;

#[cfg(target_arch = "wasm32")]
pub use web::Translator;

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;

    /// No models in the web build.
    pub struct Translator(());

    impl Translator {
        pub fn load(_dir: &Path, _target: Option<&str>) -> Result<Self> {
            Err(Error::Unavailable)
        }

        pub fn translate(&mut self, _text: &str, _cancel: &AtomicBool) -> Result<String> {
            Err(Error::Unavailable)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::sync::atomic::Ordering;

    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::marian::{Config, MTModel};

    use super::*;

    fn err(e: candle_core::Error) -> Error {
        Error::Inference(e.to_string())
    }

    /// A loaded translation model.
    pub struct Translator {
        model: MTModel,
        config: Config,
        source: spm::Model,
        vocab: Vocab,
        target_token: Option<u32>,
        unk: u32,
        /// Tokens never shown in the output.
        special: Vec<u32>,
        /// Joins sentences in the target language (no space for Chinese, Japanese and Thai).
        joiner: &'static str,
    }

    impl Translator {
        /// Load the model in `dir`. `target` (a language code) picks the output language of a
        /// model into several languages; for single-pair models it may be `None`.
        pub fn load(dir: &Path, target: Option<&str>) -> Result<Self> {
            for f in crate::models::TRANSLATION_FILES {
                if !dir.join(f).is_file() {
                    return Err(Error::Model(format!("the translation model folder {} has no {f}", dir.display())));
                }
            }
            let cfg_value = config_json(&crate::read_text_file(&dir.join("config.json"))?)?;
            let config: Config =
                serde_json::from_value(cfg_value).map_err(|e| Error::Model(format!("config.json isn't a Marian configuration: {e}")))?;
            if config.vocab_size == 0
                || config.vocab_size > 1 << 20
                || config.d_model == 0
                || config.max_position_embeddings < 8
                || config.encoder_attention_heads == 0
                || config.decoder_attention_heads == 0
            {
                return Err(Error::Model("config.json has impossible sizes".into()));
            }
            let source = spm::Model::parse(&crate::read_capped(&dir.join("source.spm"), crate::MAX_TEXT_FILE_BYTES)?)?;
            // The target model must be readable too, even though decoding only needs the vocabulary.
            spm::Model::parse(&crate::read_capped(&dir.join("target.spm"), crate::MAX_TEXT_FILE_BYTES)?)?;
            let vocab = Vocab::parse(&crate::read_text_file(&dir.join("vocab.json"))?)?;
            let unk = vocab.id("<unk>").unwrap_or(1);
            let target = target.map(crate::lang::normalize);
            // Models into several languages have `>>xx<<` (or `>>xxx<<`) tokens.
            let target_token = match &target {
                Some(t) => {
                    let candidates = [format!(">>{t}<<"), format!(">>{}<<", three_letter(t))];
                    candidates.iter().find_map(|c| vocab.id(c))
                }
                None => None,
            };
            let has_lang_tokens = vocab.ids.keys().any(|k| k.starts_with(">>") && k.ends_with("<<"));
            if has_lang_tokens && target_token.is_none() {
                return Err(Error::Model(format!(
                    "this model translates into several languages, but not into {}",
                    target.as_deref().map(crate::lang::name).unwrap_or_else(|| "the one asked for".into())
                )));
            }
            let weights =
                crate::models::translation_weights(dir).ok_or_else(|| Error::Model(format!("{} has no model.safetensors", dir.display())))?;
            let size = std::fs::metadata(&weights).map(|m| m.len()).unwrap_or(0);
            if size > crate::MAX_WEIGHTS_BYTES {
                return Err(Error::Model(format!("{} is too large ({} MB)", weights.display(), size >> 20)));
            }
            let device = Device::Cpu;
            let model = crate::guarded(|| {
                let mut tensors: HashMap<String, Tensor> = if weights.extension().is_some_and(|e| e == "safetensors") {
                    candle_core::safetensors::load(&weights, &device).map_err(|e| Error::Model(format!("can't read {}: {e}", weights.display())))?
                } else {
                    candle_core::pickle::read_all(&weights)
                        .map_err(|_| {
                            Error::Model(format!(
                                "can't read {} (an old PyTorch format): use the model's model.safetensors file instead",
                                weights.display()
                            ))
                        })?
                        .into_iter()
                        .collect()
                };
                // Files saved with shared weights keep them under one of their names.
                if !tensors.contains_key("model.shared.weight")
                    && let Some(t) = ["model.encoder.embed_tokens.weight", "model.decoder.embed_tokens.weight", "lm_head.weight"]
                        .iter()
                        .find_map(|k| tensors.get(*k).cloned())
                {
                    tensors.insert("model.shared.weight".into(), t);
                }
                if !tensors.contains_key("final_logits_bias") {
                    let n = config.decoder_vocab_size.unwrap_or(config.vocab_size);
                    tensors.insert("final_logits_bias".into(), Tensor::zeros((1, n), DType::F32, &device).map_err(err)?);
                }
                let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
                MTModel::new(&config, vb).map_err(|e| Error::Model(format!("the weights don't fit config.json: {e}")))
            })?;
            let special = vec![config.eos_token_id, config.pad_token_id, config.forced_eos_token_id, unk];
            let joiner = if target.as_deref().is_some_and(|t| matches!(t, "zh" | "ja" | "th")) { "" } else { " " };
            Ok(Translator { model, config, source, vocab, target_token, unk, special, joiner })
        }

        /// Translate `text`, sentence by sentence; whitespace around it is kept. Checks `cancel`
        /// between steps.
        pub fn translate(&mut self, text: &str, cancel: &AtomicBool) -> Result<String> {
            let (lead, core, trail) = crate::text::trim_parts(text);
            if core.is_empty() {
                return Ok(text.to_string());
            }
            let mut out: Vec<String> = Vec::new();
            for sentence in crate::text::sentences(core) {
                for part in crate::text::split_long(sentence, LONG_SENTENCE_CHARS) {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(Error::Cancelled);
                    }
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    // Nothing to translate in numbers and punctuation.
                    if !part.chars().any(char::is_alphabetic) {
                        out.push(part.to_string());
                        continue;
                    }
                    let t = crate::guarded(|| self.translate_one(part, cancel))?;
                    out.push(t);
                }
            }
            Ok(format!("{lead}{}{trail}", out.join(self.joiner)))
        }

        fn translate_one(&mut self, text: &str, cancel: &AtomicBool) -> Result<String> {
            let device = Device::Cpu;
            let input = encode(&self.source, &self.vocab, text, self.target_token, self.config.eos_token_id, self.unk);
            let tokens = Tensor::new(input.as_slice(), &device).and_then(|t| t.unsqueeze(0)).map_err(err)?;
            self.model.reset_kv_cache();
            let encoded = self.model.encoder().forward(&tokens, 0).map_err(err)?;
            let max_out = (input.len() * 3 + 16).min(self.config.max_position_embeddings.saturating_sub(1));
            let mut out = vec![self.config.decoder_start_token_id];
            let pad = self.config.pad_token_id as usize;
            for step in 0..max_out {
                if cancel.load(Ordering::Relaxed) {
                    self.model.reset_kv_cache();
                    return Err(Error::Cancelled);
                }
                let context = if step == 0 { out.len() } else { 1 };
                let start = out.len().saturating_sub(context);
                let ids = Tensor::new(out.get(start..).unwrap_or_default(), &device).and_then(|t| t.unsqueeze(0)).map_err(err)?;
                let logits = self.model.decode(&ids, &encoded, start).map_err(err)?;
                let logits = logits.squeeze(0).map_err(err)?;
                let last = logits.dim(0).map_err(err)?.saturating_sub(1);
                let mut values: Vec<f32> = logits.get(last).and_then(|l| l.to_vec1()).map_err(err)?;
                // The padding token is never an answer (HF's `bad_words_ids`).
                if let Some(v) = values.get_mut(pad) {
                    *v = f32::NEG_INFINITY;
                }
                let Some((next, _)) = values.iter().enumerate().filter(|(_, v)| v.is_finite()).max_by(|a, b| a.1.total_cmp(b.1)) else { break };
                let next = next as u32;
                if next == self.config.eos_token_id || next == self.config.forced_eos_token_id {
                    break;
                }
                out.push(next);
            }
            self.model.reset_kv_cache();
            Ok(decode(&self.vocab, out.get(1..).unwrap_or_default(), &self.special))
        }
    }
}

/// The three-letter code of a language, for models whose target tokens use them.
fn three_letter(code: &str) -> &'static str {
    match code {
        "ar" => "ara",
        "de" => "deu",
        "en" => "eng",
        "es" => "spa",
        "fr" => "fra",
        "he" => "heb",
        "it" => "ita",
        "ja" => "jpn",
        "ko" => "kor",
        "nl" => "nld",
        "pl" => "pol",
        "pt" => "por",
        "ru" => "rus",
        "tr" => "tur",
        "uk" => "ukr",
        "zh" => "zho",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_maps_to_vocabulary_ids_and_back() {
        let sp = crate::spm::tests::english();
        let vocab = Vocab::parse(r#"{"</s>": 0, "<unk>": 1, "▁the": 2, "▁cat": 3, "s": 4, ".": 5, ">>he<<": 6, "<pad>": 7}"#).unwrap();
        assert_eq!(encode(&sp, &vocab, "the cats.", None, 0, 1), vec![2, 3, 4, 5, 0]);
        // Unknown pieces become <unk>; a target token goes first.
        assert_eq!(encode(&sp, &vocab, "the €", Some(6), 0, 1), vec![6, 2, 1, 1, 0]);
        assert_eq!(decode(&vocab, &[2, 3, 4, 5, 0, 7], &[0, 7, 1]), "the cats.");
        assert_eq!(decode(&vocab, &[99], &[]), "");
        // Very long input is cut to what the model takes.
        assert_eq!(encode(&sp, &vocab, &"the ".repeat(1000), None, 0, 1).len(), MAX_INPUT_TOKENS + 1);
    }

    #[test]
    fn config_defaults_and_refusals() {
        let v = config_json(r#"{"model_type": "marian", "vocab_size": 10, "eos_token_id": 0}"#).unwrap();
        assert_eq!(v["decoder_vocab_size"], 10);
        assert_eq!(v["forced_eos_token_id"], 0);
        assert_eq!(v["share_encoder_decoder_embeddings"], true);
        assert!(config_json(r#"{"model_type": "bart", "vocab_size": 10}"#).is_err());
        assert!(config_json(r#"{"model_type": "marian"}"#).is_err());
        assert!(config_json(r#"{"vocab_size": 10, "share_encoder_decoder_embeddings": false}"#).is_err());
        assert!(config_json("[]").is_err());
        assert!(Vocab::parse("{}").is_err());
        assert!(Vocab::parse(r#"{"a": 99999999}"#).is_err());
    }
}
