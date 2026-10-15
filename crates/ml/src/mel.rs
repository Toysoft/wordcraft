//! Whisper's mel filter bank, computed rather than shipped: triangular filters on the Slaney mel
//! scale with Slaney area normalisation, over the 201 bins of a 400-point FFT at 16 kHz — the
//! standard construction (Slaney's Auditory Toolbox, 1998) that Whisper's front end uses.

/// FFT size Whisper uses.
pub const N_FFT: usize = 400;

/// Hertz to mels, Slaney's scale: linear below 1 kHz, logarithmic above.
fn hz_to_mel(f: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if f >= min_log_hz { min_log_mel + (f / min_log_hz).ln() / logstep } else { f / f_sp }
}

fn mel_to_hz(m: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4f64.ln() / 27.0;
    if m >= min_log_mel { min_log_hz * (logstep * (m - min_log_mel)).exp() } else { f_sp * m }
}

/// The filter bank for `n_mels` bands (80 for most Whisper models, 128 for large-v3), row-major:
/// `n_mels` rows of `1 + N_FFT / 2` weights. `n_mels` is clamped to 1..=256.
pub fn filters(n_mels: usize, sample_rate: u32) -> Vec<f32> {
    let n_mels = n_mels.clamp(1, 256);
    let bins = 1 + N_FFT / 2;
    let sr = f64::from(sample_rate.max(1));
    let fft_freqs: Vec<f64> = (0..bins).map(|i| i as f64 * sr / N_FFT as f64).collect();
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(sr / 2.0));
    let mel_f: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (n_mels + 1) as f64)).collect();
    let mut out = vec![0f32; n_mels * bins];
    for (m, row) in out.chunks_exact_mut(bins).enumerate() {
        let (Some(&left), Some(&center), Some(&right)) = (mel_f.get(m), mel_f.get(m + 1), mel_f.get(m + 2)) else { continue };
        let (down, up) = (center - left, right - center);
        if down <= 0.0 || up <= 0.0 {
            continue;
        }
        let norm = 2.0 / (right - left);
        for (w, f) in row.iter_mut().zip(&fft_freqs) {
            let lower = (f - left) / down;
            let upper = (right - f) / up;
            *w = (lower.min(upper).max(0.0) * norm) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_scale_round_trips() {
        for f in [0.0, 300.0, 999.0, 1000.0, 4000.0, 8000.0] {
            assert!((mel_to_hz(hz_to_mel(f)) - f).abs() < 1e-6, "{f}");
        }
        // Linear below 1 kHz: 1000 Hz is mel 15.
        assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn filters_are_triangles_covering_the_spectrum() {
        let f = filters(80, 16_000);
        assert_eq!(f.len(), 80 * 201);
        assert!(f.iter().all(|w| w.is_finite() && *w >= 0.0));
        for (m, row) in f.as_chunks::<201>().0.iter().enumerate() {
            let peak = row.iter().enumerate().fold((0, 0f32), |a, (i, w)| if *w > a.1 { (i, *w) } else { a });
            assert!(peak.1 > 0.0, "band {m} is empty");
            // Rises to its peak, then falls.
            assert!(row[..peak.0].windows(2).all(|w| w[0] <= w[1]), "band {m}");
            assert!(row[peak.0..].windows(2).all(|w| w[0] >= w[1]), "band {m}");
        }
        // Bands move up the spectrum.
        let first_bin = |m: usize| f[m * 201..(m + 1) * 201].iter().position(|w| *w > 0.0).unwrap();
        assert!(first_bin(79) > first_bin(40) && first_bin(40) > first_bin(1));
        // The lowest band's weight at its peak (Slaney normalisation: 2 / bandwidth in Hz).
        assert!((f.iter().take(201).copied().fold(0f32, f32::max) - 0.0247).abs() < 0.002);
    }

    #[test]
    fn odd_sizes_never_panic() {
        assert_eq!(filters(0, 16_000).len(), 201);
        assert_eq!(filters(10_000, 0).len(), 256 * 201);
        assert_eq!(filters(128, 16_000).len(), 128 * 201);
    }
}
