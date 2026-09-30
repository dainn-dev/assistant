//! Shared sample-rate conversion for all capture backends.
//!
//! Every backend must emit PCM s16le mono at `TARGET_SAMPLE_RATE`, but sources
//! deliver different native formats/rates. Integer-ratio downsampling
//! (e.g. 48 kHz -> 16 kHz) uses a Hamming-windowed-sinc FIR decimator so
//! content above the output Nyquist is attenuated instead of aliasing back;
//! other ratios use a streaming linear interpolator. Both carry state across
//! `process` calls so chunk boundaries stay continuous.

use std::collections::VecDeque;

/// Streaming sample-rate converter.
pub enum Resampler {
    /// Input already at the target rate.
    Pass,
    /// Integer decimation with an anti-alias FIR filter.
    Decimate(FirDecimator),
    /// Arbitrary-ratio linear interpolation (upsampling or non-integer ratios).
    Linear(LinearResampler),
}

impl Resampler {
    pub fn new(source_rate: u32, target_rate: u32) -> Self {
        if source_rate == 0 || target_rate == 0 || source_rate == target_rate {
            return Resampler::Pass;
        }
        if source_rate > target_rate && source_rate.is_multiple_of(target_rate) {
            Resampler::Decimate(FirDecimator::new((source_rate / target_rate) as usize))
        } else {
            Resampler::Linear(LinearResampler::new(source_rate, target_rate))
        }
    }

    /// Convert one chunk of mono input samples. Internal state carries over so
    /// consecutive calls produce a seamless stream.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        match self {
            Resampler::Pass => input.to_vec(),
            Resampler::Decimate(d) => d.process(input),
            Resampler::Linear(l) => l.process(input),
        }
    }
}

/// FIR decimator: low-pass filter (cutoff just under the output Nyquist)
/// followed by taking every `ratio`-th sample.
pub struct FirDecimator {
    taps: Vec<f32>,
    /// Sliding window of the last `taps.len()` input samples.
    hist: VecDeque<f32>,
    /// Input samples until the next output sample.
    phase: usize,
    ratio: usize,
}

impl FirDecimator {
    pub fn new(ratio: usize) -> Self {
        let taps = lowpass_taps(ratio.max(1));
        let hist = VecDeque::from(vec![0.0; taps.len()]);
        Self {
            taps,
            hist,
            phase: 0,
            ratio: ratio.max(1),
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(input.len() / self.ratio + 1);
        for &s in input {
            self.hist.pop_front();
            self.hist.push_back(s);
            if self.phase == 0 {
                out.push(
                    self.hist
                        .iter()
                        .zip(self.taps.iter())
                        .map(|(x, t)| x * t)
                        .sum(),
                );
                self.phase = self.ratio - 1;
            } else {
                self.phase -= 1;
            }
        }
        out
    }
}

/// Odd-length symmetric Hamming-windowed sinc, normalized to unity gain.
fn lowpass_taps(ratio: usize) -> Vec<f32> {
    // ~8 taps per decimation step: decent stop-band attenuation (~40 dB)
    // with negligible real-time latency (25 taps at 48->16 kHz).
    let len = ratio * 8 + 1;
    let m = (len - 1) as f32 / 2.0;
    // Cut just below output Nyquist to leave a small transition band.
    let fc = 0.45 / ratio as f32; // cycles per input sample
    let mut taps: Vec<f32> = (0..len)
        .map(|i| {
            let x = i as f32 - m;
            let sinc = if x == 0.0 {
                1.0
            } else {
                let arg = std::f32::consts::PI * 2.0 * fc * x;
                arg.sin() / arg
            };
            let w = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (len - 1) as f32).cos();
            2.0 * fc * sinc * w
        })
        .collect();
    let sum: f32 = taps.iter().sum();
    if sum != 0.0 {
        for t in &mut taps {
            *t /= sum;
        }
    }
    taps
}

/// Streaming linear interpolator for arbitrary ratios. The "extended input"
/// of each chunk is `pending` (last sample of the previous chunk) followed by
/// `input`, so interpolation is continuous across chunk boundaries.
pub struct LinearResampler {
    /// Input samples consumed per output sample (source_rate / target_rate).
    step: f64,
    /// Position of the next output sample in extended-input coordinates.
    frac: f64,
    /// Last input sample of the previous chunk.
    pending: f32,
}

impl LinearResampler {
    pub fn new(source_rate: u32, target_rate: u32) -> Self {
        Self {
            step: source_rate as f64 / target_rate as f64,
            frac: 0.0,
            pending: 0.0,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        let l = input.len();
        // Tiny chunks with a large step could leave frac beyond this chunk's
        // reach; clamp so the loop can always make progress.
        self.frac = self.frac.min(l as f64);

        let mut out = Vec::with_capacity((l as f64 / self.step) as usize + 2);
        let at = |i: usize| if i == 0 { self.pending } else { input[i - 1] };
        // Emit while interpolation needs no sample beyond input[l-1].
        while self.frac < l as f64 {
            let i = self.frac.floor() as usize;
            let t = (self.frac - i as f64) as f32;
            out.push(at(i) + (at(i + 1) - at(i)) * t);
            self.frac += self.step;
        }
        self.pending = input[l - 1];
        self.frac -= l as f64;
        out
    }
}

/// Downmix interleaved multi-channel f32 audio to mono.
pub fn mixdown_f32(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Downmix interleaved multi-channel i16 audio to mono f32.
pub fn mixdown_i16(data: &[i16], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.iter().map(|&x| x as f32 / 32768.0).collect();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / channels as f32)
        .collect()
}

/// Pack f32 samples into little-endian s16 PCM bytes.
pub fn f32_to_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        out.extend_from_slice(&((clamped * 32767.0) as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
    }

    #[test]
    fn passthrough_when_rates_match() {
        let mut r = Resampler::new(16000, 16000);
        let input = vec![0.1f32, -0.5, 0.9];
        assert_eq!(r.process(&input), input);
    }

    #[test]
    fn decimator_attenuates_above_nyquist() {
        // 20 kHz input at 48 kHz is above the 8 kHz output Nyquist: without a
        // filter it would alias back as audible energy. After FIR decimation
        // it must be strongly attenuated.
        let mut r = Resampler::new(48000, 16000);
        let input = sine(20_000.0, 48000, 4800);
        let out = r.process(&input);
        let tail = &out[out.len() / 4..];
        assert!(
            rms(tail) < rms(&input) * 0.2,
            "aliased energy not attenuated: in={} out={}",
            rms(&input),
            rms(tail)
        );
    }

    #[test]
    fn decimator_preserves_in_band_signal() {
        let mut r = Resampler::new(48000, 16000);
        let input = sine(1_000.0, 48000, 4800);
        let out = r.process(&input);
        // Output length: 4800 / 3 = 1600 (plus/minus filter warmup edge)
        assert!(
            (out.len() as i64 - 1600).abs() <= 2,
            "unexpected output length {}",
            out.len()
        );
        let tail = &out[out.len() / 2..];
        assert!(
            rms(tail) > rms(&input) * 0.5,
            "in-band signal attenuated: in={} out={}",
            rms(&input),
            rms(tail)
        );
    }

    #[test]
    fn linear_resampler_is_continuous_across_chunks() {
        let input = sine(500.0, 44100, 4410);
        let whole = {
            let mut r = Resampler::new(44100, 16000);
            r.process(&input)
        };
        let mut chunked = Vec::new();
        {
            let mut r = Resampler::new(44100, 16000);
            for c in input.chunks(1024) {
                chunked.extend(r.process(c));
            }
        }
        assert!(
            whole.len().abs_diff(chunked.len()) <= 2,
            "chunked={} whole={}",
            chunked.len(),
            whole.len()
        );
        let n = whole.len().min(chunked.len());
        let max_diff = whole[..n]
            .iter()
            .zip(&chunked[..n])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_diff < 1e-4, "chunk boundary discontinuity {max_diff}");
    }

    #[test]
    fn mixdown_averages_channels() {
        let stereo = vec![1.0f32, -1.0, 0.5, 0.5];
        let mono = mixdown_f32(&stereo, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }
}
