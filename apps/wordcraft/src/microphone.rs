//! The microphone for Dictate: the system's default input device through cpal (Core Audio on
//! macOS, WASAPI on Windows, ALSA on Linux and the BSDs). The audio callback runs on cpal's
//! thread and only appends samples to a shared buffer; the window takes them each frame and hands
//! them to the transcribing worker (`wordcraft_ui_egui::local_ai`). Dropping the recording stops it.

use std::sync::{Arc, Mutex, PoisonError};

use cpal::Sample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use wordcraft_ui_egui::local_ai::Microphone;

/// Most audio kept waiting for the window, in samples (a minute of 48 kHz stereo): if the window
/// stalls longer, the oldest audio goes.
const MAX_PENDING: usize = 48_000 * 2 * 60;

#[derive(Default)]
struct Shared {
    samples: Vec<f32>,
    error: Option<String>,
}

struct Recording {
    /// Recording while it lives.
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    rate: u32,
    channels: u16,
}

impl Microphone for Recording {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn take(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.shared.lock().unwrap_or_else(PoisonError::into_inner).samples)
    }

    fn error(&self) -> Option<String> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner).error.clone()
    }
}

/// Add samples from the audio callback.
fn push(shared: &Mutex<Shared>, samples: impl Iterator<Item = f32>) {
    let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
    s.samples.extend(samples.map(|v| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }));
    let excess = s.samples.len().saturating_sub(MAX_PENDING);
    if excess > 0 {
        s.samples.drain(..excess);
    }
}

/// Start recording from the default microphone.
pub fn open() -> Result<Box<dyn Microphone>, String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(|| "no microphone was found".to_string())?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    let shared = Arc::new(Mutex::new(Shared::default()));
    let on_error = {
        let shared = shared.clone();
        move |e: cpal::Error| shared.lock().unwrap_or_else(PoisonError::into_inner).error = Some(e.to_string())
    };
    macro_rules! build {
        ($t:ty) => {{
            let shared = shared.clone();
            device.build_input_stream::<$t, _, _>(
                config.clone(),
                move |data: &[$t], _: &cpal::InputCallbackInfo| push(&shared, data.iter().map(|s| s.to_sample::<f32>())),
                on_error,
                None,
            )
        }};
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::I8 => build!(i8),
        cpal::SampleFormat::U8 => build!(u8),
        cpal::SampleFormat::F64 => build!(f64),
        other => return Err(format!("the microphone's sample format ({other:?}) isn't supported")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    log::info!("dictation: recording at {} Hz, {} channel(s), {:?}", config.sample_rate, config.channels, supported.sample_format());
    Ok(Box::new(Recording { _stream: stream, shared, rate: config.sample_rate, channels: config.channels }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_audio_is_capped_and_cleaned() {
        let shared = Mutex::new(Shared::default());
        push(&shared, [f32::NAN, 2.0, -0.5].into_iter());
        assert_eq!(shared.lock().unwrap().samples, vec![0.0, 1.0, -0.5]);
        push(&shared, std::iter::repeat_n(0.1, MAX_PENDING + 10));
        let s = shared.lock().unwrap();
        assert_eq!(s.samples.len(), MAX_PENDING);
        assert!(s.samples.iter().all(|v| *v == 0.1));
    }
}
