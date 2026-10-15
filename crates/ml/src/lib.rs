//! Local models for WordCraft: speech recognition for Dictate and machine translation for
//! Translate, run offline on the computer's CPU. Nothing is sent anywhere and nothing is
//! downloaded: the user points WordCraft at model files they already have (File › Options ›
//! Language), and `docs/local-models.md` says where to get them.
//!
//! - **Speech:** OpenAI's Whisper (MIT weights), in the Hugging Face layout: a folder with
//!   `config.json`, `tokenizer.json` and `model.safetensors`. Multilingual models detect the
//!   spoken language or take one ([`SpeechModel`]).
//! - **Translation:** Marian models such as Helsinki-NLP's OPUS-MT (CC-BY 4.0 / Apache-2.0), one
//!   language pair per folder: `config.json`, `source.spm`, `target.spm`, `vocab.json` and
//!   `model.safetensors` ([`Translator`], [`models::scan_translation_models`]).
//!
//! The networks run on [candle](https://github.com/huggingface/candle) (pure Rust). The parts
//! around them are here and are plain functions that tests run without any model: microphone
//! audio to 16 kHz mono and pause detection ([`audio`]), Whisper's mel filters ([`mel`]), its
//! byte-level tokens ([`bpe`]), SentencePiece ([`spm`]) and sentence splitting ([`text`]).
//!
//! Model files are untrusted input like any document: a damaged or unexpected file is an
//! [`Error`], never a crash. Inference runs inside a panic guard as well, because the numeric
//! code below this crate indexes freely.
//!
//! The web build has no models: [`AVAILABLE`] is false there and loading says so.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod audio;
pub mod bpe;
pub mod lang;
pub mod mel;
pub mod models;
pub mod spm;
pub mod text;

mod speech;
mod translate;

pub use speech::{SpeechModel, Transcript};
pub use translate::Translator;

/// Whether this build can run models (desktop: yes; the web build: no).
pub const AVAILABLE: bool = cfg!(not(target_arch = "wasm32"));

/// Largest model weights file read, in bytes (Whisper large in 16-bit floats is about 3 GB).
pub const MAX_WEIGHTS_BYTES: u64 = 4 << 30;
/// Largest tokenizer, vocabulary or configuration file read, in bytes.
pub const MAX_TEXT_FILE_BYTES: u64 = 64 << 20;

/// What went wrong.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// The model folder is missing a file, or a file isn't what it should be.
    #[error("{0}")]
    Model(String),
    /// The audio couldn't be read.
    #[error("{0}")]
    Audio(String),
    /// The model failed while running.
    #[error("the model failed: {0}")]
    Inference(String),
    /// The user stopped it.
    #[error("cancelled")]
    Cancelled,
    /// This build has no models (the web version).
    #[error("local models aren't available in the web version of WordCraft")]
    Unavailable,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Read a small text file of a model (configuration, tokenizer), refusing huge ones.
pub(crate) fn read_text_file(path: &std::path::Path) -> Result<String> {
    let bytes = read_capped(path, MAX_TEXT_FILE_BYTES)?;
    String::from_utf8(bytes).map_err(|_| Error::Model(format!("{} isn't UTF-8 text", path.display())))
}

/// Read a file of at most `cap` bytes.
pub(crate) fn read_capped(path: &std::path::Path, cap: u64) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|e| Error::Model(format!("can't read {}: {e}", path.display())))?;
    if !meta.is_file() {
        return Err(Error::Model(format!("{} isn't a file", path.display())));
    }
    if meta.len() > cap {
        return Err(Error::Model(format!("{} is too large ({} MB)", path.display(), meta.len() >> 20)));
    }
    std::fs::read(path).map_err(|e| Error::Model(format!("can't read {}: {e}", path.display())))
}

/// Run `f`, turning a panic in the numeric code into an error instead of a crash.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let why = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "internal error".into());
            log::error!("model panicked: {why}");
            Err(Error::Inference(why))
        }
    }
}
