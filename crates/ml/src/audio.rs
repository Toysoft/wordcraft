//! Audio for speech recognition: WAV files, any microphone format to the 16 kHz mono that
//! Whisper hears, and finding the pauses in live speech so each phrase is transcribed as soon as
//! the speaker stops.

use crate::{Error, Result};

/// The sample rate Whisper hears.
pub const SAMPLE_RATE: u32 = 16_000;
/// Longest audio taken in one go, in seconds (an hour): longer input is cut there.
pub const MAX_SECONDS: usize = 3600;
/// Most channels a WAV file or microphone may have.
const MAX_CHANNELS: u16 = 64;

/// Interleaved samples in -1.0..=1.0, with their rate and channel count.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: u16,
}

impl Pcm {
    /// The audio as 16 kHz mono, at most [`MAX_SECONDS`] long.
    pub fn to_whisper(&self) -> Vec<f32> {
        resample(&to_mono(&self.samples, self.channels), self.rate, SAMPLE_RATE)
    }
}

/// Mix interleaved frames down to one channel (0 channels counts as 1). Samples that aren't
/// finite count as silence.
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let clean = |s: &f32| if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 };
    let n = usize::from(channels.clamp(1, MAX_CHANNELS));
    if n == 1 {
        return samples.iter().map(clean).collect();
    }
    samples.chunks(n).map(|frame| frame.iter().map(clean).sum::<f32>() / frame.len().max(1) as f32).collect()
}

/// One output sample at input position `t` (in samples of `mono`) when converting at `ratio`
/// (input rate / output rate). Going down, a Hann-windowed sinc low-pass at 0.42 of the new
/// rate, normalised so a constant stays constant (and so the window cut short at the ends doesn't
/// dip); going up, linear interpolation.
fn sample_at(mono: &[f32], t: f64, ratio: f64) -> f32 {
    let Some(last) = mono.len().checked_sub(1) else { return 0.0 };
    if ratio > 1.0 {
        let fc = 0.42 / ratio;
        let half = reach(ratio) as f64;
        let lo = (t - half).ceil().max(0.0) as usize;
        let hi = ((t + half).floor().max(0.0) as usize).min(last);
        let (mut sum, mut weight) = (0f64, 0f64);
        for k in lo..=hi {
            let x = k as f64 - t;
            let sinc = if x.abs() < 1e-9 { 1.0 } else { (2.0 * std::f64::consts::PI * fc * x).sin() / (2.0 * std::f64::consts::PI * fc * x) };
            let w = sinc * (0.5 + 0.5 * (std::f64::consts::PI * x / half).cos());
            sum += f64::from(mono.get(k).copied().unwrap_or(0.0)) * w;
            weight += w;
        }
        if weight.abs() > 1e-9 { (sum / weight) as f32 } else { 0.0 }
    } else {
        let a = (t.floor().max(0.0) as usize).min(last);
        let b = (a + 1).min(last);
        let frac = (t - a as f64).clamp(0.0, 1.0) as f32;
        let (x, y) = (mono.get(a).copied().unwrap_or(0.0), mono.get(b).copied().unwrap_or(0.0));
        x + (y - x) * frac
    }
}

/// How many input samples on each side an output sample reads.
fn reach(ratio: f64) -> usize {
    if ratio > 1.0 { (12.0 * ratio).ceil() as usize } else { 1 }
}

/// Resample mono audio from `from` Hz to `to` Hz. Going down, the audio is low-pass filtered
/// first (so sounds above the new rate's range don't fold back into speech); going up, it is
/// interpolated. Output is capped at [`MAX_SECONDS`].
pub fn resample(mono: &[f32], from: u32, to: u32) -> Vec<f32> {
    let cap = MAX_SECONDS.saturating_mul(to as usize);
    if from == 0 || to == 0 || mono.is_empty() {
        return Vec::new();
    }
    if from == to {
        return mono.iter().take(cap).copied().collect();
    }
    let ratio = f64::from(from) / f64::from(to);
    let out_len = ((mono.len() as f64 / ratio).floor() as usize).min(cap);
    (0..out_len).map(|i| sample_at(mono, i as f64 * ratio, ratio)).collect()
}

/// Live audio in any format to 16 kHz mono, a block at a time. It gives exactly what
/// [`resample`] gives for the whole recording: an output sample waits until the input its filter
/// reads has arrived, and input is kept until no later output reads it.
#[derive(Clone, Debug)]
pub struct Stream {
    rate: u32,
    channels: u16,
    /// Mono input from absolute index `base` on.
    held: Vec<f32>,
    base: usize,
    /// Input samples seen.
    total: usize,
    /// The next output sample's index.
    next: usize,
}

impl Stream {
    pub fn new(rate: u32, channels: u16) -> Self {
        Stream { rate: rate.clamp(1_000, 384_000), channels: channels.clamp(1, MAX_CHANNELS), held: Vec::new(), base: 0, total: 0, next: 0 }
    }

    fn ratio(&self) -> f64 {
        f64::from(self.rate) / f64::from(SAMPLE_RATE)
    }

    /// Outputs while `ready(input position)` holds, then forget input no later output reads.
    fn produce(&mut self, until: usize) -> Vec<f32> {
        let ratio = self.ratio();
        let mut out = Vec::new();
        while self.next < until {
            let t = self.next as f64 * ratio - self.base as f64;
            out.push(sample_at(&self.held, t, ratio));
            self.next += 1;
        }
        let t = self.next as f64 * ratio - self.base as f64;
        let keep_from = ((t - reach(ratio) as f64).floor().max(0.0) as usize).min(self.held.len());
        self.held.drain(..keep_from);
        self.base += keep_from;
        out
    }

    /// Feed interleaved samples; returns the 16 kHz mono audio they complete.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        let mono = to_mono(interleaved, self.channels);
        self.total = self.total.saturating_add(mono.len());
        self.held.extend(mono);
        // Output i is ready once input up to i * ratio + reach has arrived.
        let ratio = self.ratio();
        let Some(available) = self.total.checked_sub(reach(ratio) + 1) else { return Vec::new() };
        let until = ((available as f64 / ratio).floor() as usize + 1).min((self.total as f64 / ratio).floor() as usize);
        self.produce(until)
    }

    /// The rest, at the end of the recording.
    pub fn finish(&mut self) -> Vec<f32> {
        let until = (self.total as f64 / self.ratio()).floor() as usize;
        let out = self.produce(until);
        self.held.clear();
        out
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// Read a WAV file: PCM at 8, 16, 24 or 32 bits, or 32/64-bit floats, any rate and channel count.
pub fn read_wav(bytes: &[u8]) -> Result<Pcm> {
    let bad = |why: &str| Error::Audio(format!("not a WAV file this can read: {why}"));
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(bad("no RIFF/WAVE header"));
    }
    let mut at = 12usize;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let body_start = at + 8;
        let body_end = body_start.saturating_add(len as usize).min(bytes.len());
        let body = bytes.get(body_start..body_end).unwrap_or_default();
        match id {
            b"fmt " => {
                let tag = u16_at(body, 0).ok_or_else(|| bad("short fmt chunk"))?;
                let channels = u16_at(body, 2).ok_or_else(|| bad("short fmt chunk"))?;
                let rate = u32_at(body, 4).ok_or_else(|| bad("short fmt chunk"))?;
                let bits = u16_at(body, 14).ok_or_else(|| bad("short fmt chunk"))?;
                // WAVE_FORMAT_EXTENSIBLE: the real format is the sub-format GUID's first two bytes.
                let tag = if tag == 0xFFFE { u16_at(body, 24).ok_or_else(|| bad("short extensible fmt chunk"))? } else { tag };
                format = Some((tag, channels, rate, bits));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are padded to an even length.
        at = body_start.saturating_add(len as usize).saturating_add(len as usize & 1);
        if at >= bytes.len() {
            break;
        }
    }
    let (tag, channels, rate, bits) = format.ok_or_else(|| bad("no fmt chunk"))?;
    let data = data.ok_or_else(|| bad("no data chunk"))?;
    if channels == 0 || channels > MAX_CHANNELS {
        return Err(bad("unsupported channel count"));
    }
    if !(1_000..=384_000).contains(&rate) {
        return Err(bad("unsupported sample rate"));
    }
    let cap = MAX_SECONDS.saturating_mul(rate as usize).saturating_mul(usize::from(channels));
    let samples: Vec<f32> = match (tag, bits) {
        (1, 8) => data.iter().take(cap).map(|b| (f32::from(*b) - 128.0) / 128.0).collect(),
        (1, 16) => data.as_chunks::<2>().0.iter().take(cap).map(|c| f32::from(i16::from_le_bytes(*c)) / 32_768.0).collect(),
        (1, 24) => data
            .as_chunks::<3>()
            .0
            .iter()
            .take(cap)
            .map(|&[a, b, c]| {
                let v = i32::from_le_bytes([0, a, b, c]) >> 8;
                v as f32 / 8_388_608.0
            })
            .collect(),
        (1, 32) => data.as_chunks::<4>().0.iter().take(cap).map(|c| i32::from_le_bytes(*c) as f32 / 2_147_483_648.0).collect(),
        (3, 32) => data.as_chunks::<4>().0.iter().take(cap).map(|c| f32::from_le_bytes(*c)).collect(),
        (3, 64) => data.as_chunks::<8>().0.iter().take(cap).map(|c| f64::from_le_bytes(*c) as f32).collect(),
        _ => return Err(bad("unsupported sample format (use PCM or float)")),
    };
    let samples = samples.into_iter().map(|s| if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 }).collect();
    Ok(Pcm { samples, rate, channels })
}

/// Write 16-bit mono PCM as a WAV file (for tests and for saving a recording).
pub fn write_wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(0xFFFF_FFFE);
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.saturating_mul(2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples.iter().take(data_len as usize / 2) {
        let v = if s.is_finite() { (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16 } else { 0 };
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Root-mean-square level of some samples (0 for none).
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Splits live 16 kHz mono audio into phrases at the speaker's pauses, so each phrase can be
/// transcribed while the next is spoken. A phrase ends after [`Segmenter::PAUSE_MS`] of quiet,
/// or when it reaches [`Segmenter::MAX_PHRASE_SECONDS`]; stretches without speech are dropped.
///
/// Speech is told from quiet by loudness against the room's noise, which the segmenter keeps
/// estimating from the quiet stretches, so a fan or a hum doesn't count as talking.
#[derive(Clone, Debug)]
pub struct Segmenter {
    /// Samples not yet a whole frame.
    pending: Vec<f32>,
    /// The phrase being heard (with a little audio from before it started).
    phrase: Vec<f32>,
    /// Recent quiet frames, kept so a phrase starts with the sound just before it.
    preroll: std::collections::VecDeque<f32>,
    noise: Option<f32>,
    speech_frames: usize,
    quiet_frames: usize,
    in_speech: bool,
}

impl Default for Segmenter {
    fn default() -> Self {
        Self::new()
    }
}

impl Segmenter {
    /// Frame length, samples (30 ms).
    pub const FRAME: usize = 480;
    /// Quiet that ends a phrase, milliseconds.
    pub const PAUSE_MS: usize = 700;
    /// Longest phrase, seconds (Whisper hears 30 seconds at a time).
    pub const MAX_PHRASE_SECONDS: usize = 25;
    /// Shortest speech that counts as a phrase, milliseconds (a click or a cough is shorter).
    pub const MIN_SPEECH_MS: usize = 240;
    /// Audio kept from before a phrase starts, milliseconds.
    const PREROLL_MS: usize = 300;
    /// Quietest level that can be speech, whatever the room's noise.
    const MIN_LEVEL: f32 = 0.008;

    pub fn new() -> Self {
        Segmenter {
            pending: Vec::new(),
            phrase: Vec::new(),
            preroll: Default::default(),
            noise: None,
            speech_frames: 0,
            quiet_frames: 0,
            in_speech: false,
        }
    }

    fn frames_for_ms(ms: usize) -> usize {
        (ms * SAMPLE_RATE as usize / 1000).div_ceil(Self::FRAME)
    }

    /// Whether a phrase is being heard now (for a "listening" indicator).
    pub fn hearing_speech(&self) -> bool {
        self.in_speech
    }

    /// Feed audio; returns the phrases it completed.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        let mut done = Vec::new();
        self.pending.extend(samples.iter().map(|s| if s.is_finite() { *s } else { 0.0 }));
        let whole = self.pending.len() / Self::FRAME * Self::FRAME;
        let frames: Vec<f32> = self.pending.drain(..whole).collect();
        for frame in frames.as_chunks::<{ Segmenter::FRAME }>().0 {
            if let Some(p) = self.frame(frame) {
                done.push(p);
            }
        }
        done
    }

    /// The phrase being heard, if it has enough speech (when dictation stops).
    pub fn flush(&mut self) -> Option<Vec<f32>> {
        let pending = std::mem::take(&mut self.pending);
        if self.in_speech {
            self.phrase.extend(pending);
        }
        self.end_phrase()
    }

    fn frame(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        let level = rms(frame);
        let noise = *self.noise.get_or_insert(level);
        let threshold = (noise * 3.0).max(Self::MIN_LEVEL);
        let loud = level > threshold;
        if !loud {
            // The room's noise follows the quiet frames slowly.
            self.noise = Some(noise * 0.95 + level * 0.05);
        }
        if !self.in_speech {
            if loud {
                self.in_speech = true;
                self.phrase.extend(self.preroll.drain(..));
                self.phrase.extend_from_slice(frame);
                self.speech_frames = 1;
                self.quiet_frames = 0;
            } else {
                self.preroll.extend(frame);
                let keep = Self::frames_for_ms(Self::PREROLL_MS) * Self::FRAME;
                while self.preroll.len() > keep {
                    self.preroll.pop_front();
                }
            }
            return None;
        }
        self.phrase.extend_from_slice(frame);
        if loud {
            self.speech_frames += 1;
            self.quiet_frames = 0;
        } else {
            self.quiet_frames += 1;
        }
        let max = Self::MAX_PHRASE_SECONDS * SAMPLE_RATE as usize;
        if self.quiet_frames >= Self::frames_for_ms(Self::PAUSE_MS) || self.phrase.len() + Self::FRAME > max {
            return self.end_phrase();
        }
        None
    }

    /// End the phrase being heard: it, if it had enough speech.
    fn end_phrase(&mut self) -> Option<Vec<f32>> {
        let phrase = std::mem::take(&mut self.phrase);
        let enough = self.speech_frames >= Self::frames_for_ms(Self::MIN_SPEECH_MS);
        self.in_speech = false;
        self.speech_frames = 0;
        self.quiet_frames = 0;
        self.preroll.clear();
        (enough && !phrase.is_empty()).then_some(phrase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32, amp: f32) -> Vec<f32> {
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        (0..n).map(|i| amp * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / SAMPLE_RATE as f32).sin()).collect()
    }

    fn quiet(seconds: f32) -> Vec<f32> {
        // A faint hiss, like a real room.
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        (0..n).map(|i| if i % 2 == 0 { 0.001 } else { -0.001 }).collect()
    }

    #[test]
    fn stereo_mixes_to_mono_and_bad_samples_are_silence() {
        assert_eq!(to_mono(&[1.0, 0.0, -1.0, -1.0], 2), vec![0.5, -1.0]);
        assert_eq!(to_mono(&[f32::NAN, 2.0], 1), vec![0.0, 1.0]);
        // A half frame at the end still counts; zero channels is mono.
        assert_eq!(to_mono(&[0.5, 0.5, 0.25], 2), vec![0.5, 0.25]);
        assert_eq!(to_mono(&[0.5], 0), vec![0.5]);
    }

    #[test]
    fn resampling_keeps_duration_and_level() {
        let one_second_48k = vec![0.5f32; 48_000];
        let r = resample(&one_second_48k, 48_000, 16_000);
        assert_eq!(r.len(), 16_000);
        assert!(r.iter().all(|s| (s - 0.5).abs() < 1e-6));
        let up = resample(&vec![0.25f32; 8_000], 8_000, 16_000);
        assert_eq!(up.len(), 16_000);
        assert!(up.iter().all(|s| (s - 0.25).abs() < 1e-6));
        // 44.1 kHz doesn't divide evenly.
        assert_eq!(resample(&vec![0.0; 44_100], 44_100, 16_000).len(), 16_000);
        assert!(resample(&[], 48_000, 16_000).is_empty());
        assert!(resample(&[1.0], 0, 16_000).is_empty());
        assert_eq!(resample(&[1.0, 0.0], 16_000, 16_000), vec![1.0, 0.0]);
    }

    #[test]
    fn downsampling_averages_away_frequencies_above_the_new_nyquist() {
        // An 8 kHz tone at 48 kHz can't be heard at 16 kHz: it must average out, not alias.
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 8_000.0 / 48_000.0).sin()).collect();
        let r = resample(&tone, 48_000, 16_000);
        assert!(rms(&r) < 0.2, "aliased: {}", rms(&r));
    }

    #[test]
    fn streaming_gives_what_resampling_the_whole_recording_gives() {
        for (rate, channels) in [(48_000u32, 2u16), (44_100, 1), (16_000, 1), (8_000, 1)] {
            let n = rate as usize * 2;
            let interleaved: Vec<f32> = (0..n * channels as usize).map(|i| ((i * 7919 % 1000) as f32 / 500.0 - 1.0) * 0.5).collect();
            let whole = resample(&to_mono(&interleaved, channels), rate, SAMPLE_RATE);
            let mut s = Stream::new(rate, channels);
            let mut out = Vec::new();
            let mut at = 0;
            for size in [1usize, 7, 480, 4410, 33, 9600].iter().cycle() {
                if at >= interleaved.len() {
                    break;
                }
                let end = (at + size * channels as usize).min(interleaved.len());
                out.extend(s.push(&interleaved[at..end]));
                at = end;
            }
            out.extend(s.finish());
            assert_eq!(out.len(), whole.len(), "{rate}");
            let worst = out.iter().zip(&whole).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            assert!(worst < 1e-5, "{rate} Hz: off by {worst}");
        }
    }

    #[test]
    fn wav_round_trip_and_formats() {
        let samples = vec![0.0, 0.5, -0.5, 1.0];
        let wav = write_wav(&samples, 16_000);
        let pcm = read_wav(&wav).unwrap();
        assert_eq!((pcm.rate, pcm.channels), (16_000, 1));
        assert_eq!(pcm.samples.len(), 4);
        assert!(pcm.samples.iter().zip(&samples).all(|(a, b)| (a - b).abs() < 1e-3));
        assert_eq!(pcm.to_whisper().len(), 4);
    }

    #[test]
    fn float_and_extensible_wav_files_read() {
        // 32-bit float, stereo, 48 kHz, with an odd-sized chunk before fmt.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF\0\0\0\0WAVE");
        b.extend_from_slice(b"LIST\x03\0\0\0abc\0");
        b.extend_from_slice(b"fmt \x28\0\0\0");
        b.extend_from_slice(&0xFFFEu16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&48_000u32.to_le_bytes());
        b.extend_from_slice(&(48_000u32 * 8).to_le_bytes());
        b.extend_from_slice(&8u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(&22u16.to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        b.extend_from_slice(&[0; 14]);
        b.extend_from_slice(b"data\x10\0\0\0");
        for v in [0.25f32, -0.25, 0.5, 0.5] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let pcm = read_wav(&b).unwrap();
        assert_eq!((pcm.rate, pcm.channels), (48_000, 2));
        assert_eq!(to_mono(&pcm.samples, pcm.channels), vec![0.0, 0.5]);
    }

    #[test]
    fn broken_wav_files_are_errors() {
        assert!(read_wav(b"").is_err());
        assert!(read_wav(b"RIFF\0\0\0\0WAVE").is_err());
        assert!(read_wav(b"RIFF\0\0\0\0WAVEfmt \xff\xff\xff\xff").is_err());
        let mut wav = write_wav(&[0.1], 16_000);
        // 12-bit PCM isn't a format we read.
        wav[34] = 12;
        assert!(read_wav(&wav).is_err());
        let mut wav = write_wav(&[0.1], 16_000);
        wav[22] = 0;
        assert!(read_wav(&wav).is_err(), "zero channels");
    }

    #[test]
    fn a_phrase_ends_at_a_pause() {
        let mut seg = Segmenter::new();
        let mut phrases = seg.push(&quiet(1.0));
        assert!(phrases.is_empty());
        phrases.extend(seg.push(&tone(1.0, 0.3)));
        assert!(phrases.is_empty() && seg.hearing_speech());
        phrases.extend(seg.push(&quiet(1.0)));
        assert_eq!(phrases.len(), 1);
        let p = &phrases[0];
        // The phrase holds the speech, a little before it and the pause.
        assert!(p.len() >= SAMPLE_RATE as usize && p.len() < 5 * SAMPLE_RATE as usize / 2, "{}", p.len());
        assert!(!seg.hearing_speech());
        // Silence alone never makes a phrase, nor does a click.
        assert!(seg.push(&quiet(3.0)).is_empty());
        assert!(seg.push(&tone(0.05, 0.5)).is_empty());
        assert!(seg.push(&quiet(1.0)).is_empty());
        assert!(seg.flush().is_none());
    }

    #[test]
    fn long_speech_is_cut_and_the_rest_flushed() {
        let mut seg = Segmenter::new();
        seg.push(&quiet(0.5));
        let phrases = seg.push(&tone(30.0, 0.3));
        assert_eq!(phrases.len(), 1);
        assert!(phrases[0].len() <= Segmenter::MAX_PHRASE_SECONDS * SAMPLE_RATE as usize);
        let rest = seg.flush().unwrap();
        assert!(rest.len() > 4 * SAMPLE_RATE as usize);
        assert!(seg.flush().is_none());
    }

    #[test]
    fn steady_noise_is_not_speech() {
        let mut seg = Segmenter::new();
        // A loud but steady hum from the start becomes the room's noise level.
        let hum: Vec<f32> = tone(5.0, 0.02);
        assert!(seg.push(&hum).len() <= 1);
        assert!(seg.push(&hum).is_empty());
    }
}
