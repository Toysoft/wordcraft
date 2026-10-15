//! Speech to text with a Whisper model.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::Result;

/// What was heard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    /// The words, trimmed; empty when nothing was said.
    pub text: String,
    /// The language spoken (two-letter code), as given or as the model detected it.
    pub language: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::SpeechModel;

#[cfg(target_arch = "wasm32")]
pub use web::SpeechModel;

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;

    /// No models in the web build.
    pub struct SpeechModel(());

    impl SpeechModel {
        pub fn load(_dir: &Path) -> Result<Self> {
            Err(crate::Error::Unavailable)
        }

        pub fn transcribe(&mut self, _pcm: &[f32], _language: Option<&str>, _cancel: &AtomicBool) -> Result<Transcript> {
            Err(crate::Error::Unavailable)
        }

        pub fn languages(&self) -> Vec<String> {
            Vec::new()
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::sync::atomic::Ordering;

    use candle_core::{D, DType, Device, IndexOp, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::whisper::{self as w, model::Whisper};

    use super::*;
    use crate::bpe::Vocab;
    use crate::{Error, lang};

    fn err(e: candle_core::Error) -> Error {
        Error::Inference(e.to_string())
    }

    /// Chance of "no speech" above which a quiet, unsure stretch gives no text (Whisper's own
    /// threshold), so silence doesn't come out as a made-up "Thank you."
    const NO_SPEECH: f32 = 0.6;
    /// Average log-probability below which the model counts as unsure.
    const UNSURE: f32 = -1.0;
    /// Loudness (RMS) below which a 30-second stretch counts as silence.
    const SILENCE: f32 = 0.002;
    /// The same token this many times in a row ends a segment (a decoding loop).
    const MAX_REPEATS: usize = 24;

    /// A loaded Whisper model.
    pub struct SpeechModel {
        model: Whisper,
        vocab: Vocab,
        filters: Vec<f32>,
        sot: u32,
        eot: u32,
        transcribe: u32,
        no_timestamps: u32,
        no_speech: Option<u32>,
        /// (code, token) of each language a multilingual model knows; empty for English-only ones.
        languages: Vec<(String, u32)>,
        /// Token ids never generated: special tokens (but the end of text) and the model's list.
        suppress: Tensor,
    }

    impl SpeechModel {
        /// Load the model in `dir` (`config.json`, `tokenizer.json`, `model.safetensors`).
        pub fn load(dir: &Path) -> Result<Self> {
            crate::models::check_speech_model(dir)?;
            let config: w::Config = serde_json::from_str(&crate::read_text_file(&dir.join("config.json"))?)
                .map_err(|e| Error::Model(format!("config.json isn't a Whisper configuration: {e}")))?;
            if !(1..=256).contains(&config.num_mel_bins) || config.vocab_size == 0 || config.vocab_size > 1 << 20 || config.max_target_positions < 8 {
                return Err(Error::Model("config.json has impossible sizes".into()));
            }
            let vocab = Vocab::from_tokenizer_json(&crate::read_text_file(&dir.join("tokenizer.json"))?)?;
            let token = |t: &str| vocab.special(t).ok_or_else(|| Error::Model(format!("tokenizer.json has no {t}")));
            let (sot, eot, transcribe, no_timestamps) =
                (token(w::SOT_TOKEN)?, token(w::EOT_TOKEN)?, token(w::TRANSCRIBE_TOKEN)?, token(w::NO_TIMESTAMPS_TOKEN)?);
            let no_speech = w::NO_SPEECH_TOKENS.iter().find_map(|t| vocab.special(t));
            let languages = vocab.languages();
            let weights = dir.join("model.safetensors");
            let size = std::fs::metadata(&weights).map(|m| m.len()).unwrap_or(0);
            if size > crate::MAX_WEIGHTS_BYTES {
                return Err(Error::Model(format!("{} is too large ({} MB)", weights.display(), size >> 20)));
            }
            let device = Device::Cpu;
            crate::guarded(|| {
                let tensors =
                    candle_core::safetensors::load(&weights, &device).map_err(|e| Error::Model(format!("can't read {}: {e}", weights.display())))?;
                let vb = VarBuilder::from_tensors(tensors, DType::F32, &device);
                let model = Whisper::load(&vb, config.clone()).map_err(|e| Error::Model(format!("the weights don't fit config.json: {e}")))?;
                let mask: Vec<f32> = (0..config.vocab_size as u32)
                    .map(|i| {
                        let special = i != eot && (i >= sot.min(eot) && vocab.is_special(i) || i > no_timestamps);
                        if special || config.suppress_tokens.contains(&i) { f32::NEG_INFINITY } else { 0.0 }
                    })
                    .collect();
                let suppress = Tensor::new(mask.as_slice(), &device).map_err(err)?;
                Ok(SpeechModel {
                    filters: crate::mel::filters(config.num_mel_bins, crate::audio::SAMPLE_RATE),
                    model,
                    vocab,
                    sot,
                    eot,
                    transcribe,
                    no_timestamps,
                    no_speech,
                    languages,
                    suppress,
                })
            })
        }

        /// The languages the model can be told to hear (empty for an English-only model).
        pub fn languages(&self) -> Vec<String> {
            self.languages.iter().map(|(c, _)| c.clone()).collect()
        }

        /// Transcribe 16 kHz mono audio. `language` (a code; `None` or `auto`: detect it) tells
        /// a multilingual model what it hears. Checks `cancel` between steps.
        pub fn transcribe(&mut self, pcm: &[f32], language: Option<&str>, cancel: &AtomicBool) -> Result<Transcript> {
            let max = crate::audio::MAX_SECONDS * crate::audio::SAMPLE_RATE as usize;
            let pcm = pcm.get(..pcm.len().min(max)).unwrap_or_default();
            if pcm.len() < crate::audio::SAMPLE_RATE as usize / 10 {
                return Ok(Transcript::default());
            }
            crate::guarded(|| self.run(pcm, language, cancel))
        }

        fn run(&mut self, pcm: &[f32], language: Option<&str>, cancel: &AtomicBool) -> Result<Transcript> {
            let device = Device::Cpu;
            let n_mels = self.model.config.num_mel_bins;
            let mel = w::audio::pcm_to_mel(&self.model.config, pcm, &self.filters);
            let frames = mel.len() / n_mels;
            let mel = Tensor::from_vec(mel, (1, n_mels, frames), &device).map_err(err)?;
            let content_frames = (pcm.len() / w::HOP_LENGTH).min(frames);
            let wanted = language.map(lang::normalize).filter(|l| !l.is_empty() && l != "auto");
            let mut lang_token = match &wanted {
                Some(code) => match self.languages.iter().find(|(c, _)| c == code) {
                    Some((_, t)) => Some(*t),
                    None if self.languages.is_empty() => None,
                    None => return Err(Error::Model(format!("this speech model doesn't know the language {code}"))),
                },
                None => None,
            };
            let mut detected = wanted.clone();
            let mut texts = Vec::new();
            let mut seek = 0;
            while seek < content_frames {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let size = w::N_FRAMES.min(frames - seek);
                // A silent stretch has nothing to transcribe (and Whisper tends to make words up
                // for silence).
                let audio = pcm.get(seek * w::HOP_LENGTH..((seek + size) * w::HOP_LENGTH).min(pcm.len())).unwrap_or_default();
                if crate::audio::rms(audio) < SILENCE {
                    seek += size;
                    continue;
                }
                let segment = mel.narrow(2, seek, size).map_err(err)?;
                let features = self.model.encoder.forward(&segment, true).map_err(err)?;
                if lang_token.is_none() && !self.languages.is_empty() {
                    let (code, token) = self.detect_language(&features)?;
                    detected = Some(code);
                    lang_token = Some(token);
                }
                let text = self.decode(&features, lang_token, cancel)?;
                if !text.is_empty() {
                    texts.push(text);
                }
                seek += size;
            }
            let joiner = if detected.as_deref().is_some_and(|l| matches!(l, "zh" | "ja" | "th")) { "" } else { " " };
            let text = texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(joiner);
            Ok(Transcript { text, language: detected.or_else(|| self.languages.is_empty().then(|| "en".to_string())) })
        }

        /// The most likely language of the audio: the language token the decoder rates highest
        /// right after the start of the transcript.
        fn detect_language(&mut self, features: &Tensor) -> Result<(String, u32)> {
            let device = Device::Cpu;
            let tokens = Tensor::new(&[[self.sot]], &device).map_err(err)?;
            let ys = self.model.decoder.forward(&tokens, features, true).map_err(err)?;
            let logits = self.model.decoder.final_linear(&ys.i(..1).map_err(err)?).map_err(err)?.i(0).map_err(err)?.i(0).map_err(err)?;
            let ids: Vec<u32> = self.languages.iter().map(|(_, t)| *t).collect();
            let ids_t = Tensor::new(ids.as_slice(), &device).map_err(err)?;
            let scores: Vec<f32> = logits.index_select(&ids_t, 0).map_err(err)?.to_vec1().map_err(err)?;
            let best = scores.iter().enumerate().filter(|(_, s)| s.is_finite()).max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
            best.and_then(|i| self.languages.get(i).cloned()).ok_or_else(|| Error::Inference("language detection failed".into()))
        }

        /// Greedy decoding of one 30-second segment.
        fn decode(&mut self, features: &Tensor, lang_token: Option<u32>, cancel: &AtomicBool) -> Result<String> {
            let device = Device::Cpu;
            let mut tokens = vec![self.sot];
            tokens.extend(lang_token);
            tokens.extend([self.transcribe, self.no_timestamps]);
            let prompt = tokens.len();
            let max_len = self.model.config.max_target_positions;
            let sample_len = max_len / 2;
            let mut sum_logprob = 0f32;
            let mut no_speech_prob = 0f32;
            for i in 0..sample_len {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let input = Tensor::new(tokens.as_slice(), &device).and_then(|t| t.unsqueeze(0)).map_err(err)?;
                let ys = self.model.decoder.forward(&input, features, i == 0).map_err(err)?;
                if i == 0
                    && let Some(ns) = self.no_speech
                {
                    let first = self.model.decoder.final_linear(&ys.i(..1).map_err(err)?).map_err(err)?.i(0).map_err(err)?.i(0).map_err(err)?;
                    let probs = candle_nn::ops::softmax(&first, D::Minus1).map_err(err)?;
                    no_speech_prob = probs.i(ns as usize).and_then(|p| p.to_scalar::<f32>()).map_err(err)?;
                }
                let (_, seq_len, _) = ys.dims3().map_err(err)?;
                let logits =
                    self.model.decoder.final_linear(&ys.i((..1, seq_len - 1..)).map_err(err)?).map_err(err)?.i(0).map_err(err)?.i(0).map_err(err)?;
                let logits = logits.broadcast_add(&self.suppress).map_err(err)?;
                let values: Vec<f32> = logits.to_vec1().map_err(err)?;
                let Some((next, _)) = values.iter().enumerate().filter(|(_, v)| v.is_finite()).max_by(|a, b| a.1.total_cmp(b.1)) else { break };
                let next = next as u32;
                let log_probs: Vec<f32> = candle_nn::ops::log_softmax(&logits, D::Minus1).and_then(|l| l.to_vec1()).map_err(err)?;
                sum_logprob += log_probs.get(next as usize).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                if next == self.eot || tokens.len() + 1 >= max_len {
                    break;
                }
                tokens.push(next);
                let run = tokens.iter().rev().take_while(|t| **t == next).count();
                if run >= MAX_REPEATS {
                    break;
                }
            }
            let generated = tokens.get(prompt..).unwrap_or_default();
            let avg = sum_logprob / (generated.len() + 1) as f32;
            if no_speech_prob > NO_SPEECH && avg < UNSURE {
                return Ok(String::new());
            }
            Ok(self.vocab.decode(generated).trim().to_string())
        }
    }
}
