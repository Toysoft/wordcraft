//! Checks with real model weights, which aren't in the repository: each test skips (with a
//! message) unless its environment variable points at the files.
//!
//! - `WORDCRAFT_WHISPER_MODEL`: a Whisper model folder (`config.json`, `tokenizer.json`,
//!   `model.safetensors`), e.g. `openai/whisper-tiny` from Hugging Face. With
//!   `WORDCRAFT_SPEECH_WAV` (a recording) and `WORDCRAFT_SPEECH_EXPECT` (a word it contains), the
//!   recording is transcribed too.
//! - `WORDCRAFT_TRANSLATION_MODELS`: a folder holding `opus-mt-en-he` (or another `opus-mt-en-*`
//!   model) in the Hugging Face layout with `model.safetensors`.
// Integration tests: unwrapping and panicking on failure is fine here, unlike in shipped code (AGENTS.md).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_dir())
}

#[test]
fn whisper_transcribes_a_recording_and_hears_nothing_in_silence() {
    let Some(dir) = env_dir("WORDCRAFT_WHISPER_MODEL") else {
        eprintln!("skipped: set WORDCRAFT_WHISPER_MODEL to a Whisper model folder");
        return;
    };
    let mut model = wordcraft_ml::SpeechModel::load(&dir).unwrap();
    let cancel = AtomicBool::new(false);
    let silence = vec![0.0f32; 16_000 * 3];
    let t = model.transcribe(&silence, Some("en"), &cancel).unwrap();
    eprintln!("silence: {:?}", t.text);
    assert_eq!(t.text, "", "silence came out as words");
    if let (Some(wav), Ok(expect)) = (std::env::var_os("WORDCRAFT_SPEECH_WAV"), std::env::var("WORDCRAFT_SPEECH_EXPECT")) {
        let pcm = wordcraft_ml::audio::read_wav(&std::fs::read(wav).unwrap()).unwrap();
        let started = std::time::Instant::now();
        let t = model.transcribe(&pcm.to_whisper(), None, &cancel).unwrap();
        eprintln!("heard {:?} ({:?}) in {:?}", t.text, t.language, started.elapsed());
        assert!(t.text.to_lowercase().contains(&expect.to_lowercase()), "{:?} doesn't contain {expect:?}", t.text);
    }
    // Cancelling stops it.
    let t = model.transcribe(&silence, None, &AtomicBool::new(true));
    assert_eq!(t, Err(wordcraft_ml::Error::Cancelled));
}

#[test]
fn opus_mt_translates_sentences() {
    let Some(dir) = env_dir("WORDCRAFT_TRANSLATION_MODELS") else {
        eprintln!("skipped: set WORDCRAFT_TRANSLATION_MODELS to a folder of opus-mt models");
        return;
    };
    let models = wordcraft_ml::models::scan_translation_models(&dir);
    let Some(m) = models.iter().find(|m| m.from == "en" && !m.multi_target()) else {
        eprintln!("skipped: no opus-mt-en-* model in {}", dir.display());
        return;
    };
    let mut t = wordcraft_ml::Translator::load(&m.dir, Some(&m.to)).unwrap();
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let out = t.translate("  Good morning. How are you today?  ", &cancel).unwrap();
    eprintln!("{} → {}: {out:?} in {:?}", m.from, m.to, started.elapsed());
    assert!(out.starts_with("  ") && out.ends_with("  "), "{out:?}");
    assert!(out.trim().chars().filter(|c| c.is_alphabetic()).count() > 5, "{out:?}");
    assert_ne!(out.trim(), "Good morning. How are you today?");
    if m.to == "he" {
        assert!(out.chars().any(|c| ('\u{05D0}'..='\u{05EA}').contains(&c)), "no Hebrew letters in {out:?}");
    }
    assert_eq!(t.translate("2024 — 3.5%", &cancel).unwrap(), "2024 — 3.5%");
    assert_eq!(t.translate("Hello", &AtomicBool::new(true)), Err(wordcraft_ml::Error::Cancelled));
}
