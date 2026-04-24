use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use realfft::RealFftPlanner;

use crate::PrcFmt;
use crate::audiochunk::AudioChunk;

pub const RING_BUFFER_CAPACITY: usize = 262144;

/// Circular ring buffer storing the last [`RING_BUFFER_CAPACITY`] frames per channel.
/// Self-sizes on first push; resets if channel count changes.
#[derive(Clone, Debug)]
pub struct AudioRingBuffer {
    channels: Vec<Vec<PrcFmt>>,
    write_pos: usize,
    total_written: usize,
}

impl AudioRingBuffer {
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            write_pos: 0,
            total_written: 0,
        }
    }

    pub fn push_chunk(&mut self, chunk: &AudioChunk) {
        let n_frames = chunk.valid_frames;
        let n_ch = chunk.channels;
        if self.channels.len() != n_ch {
            self.channels = vec![vec![PrcFmt::default(); RING_BUFFER_CAPACITY]; n_ch];
            self.write_pos = 0;
            self.total_written = 0;
        }
        for frame in 0..n_frames {
            for ch in 0..n_ch {
                self.channels[ch][self.write_pos] = chunk.waveforms[ch][frame];
            }
            self.write_pos = (self.write_pos + 1) % RING_BUFFER_CAPACITY;
        }
        self.total_written += n_frames;
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Copy the last `n_frames` samples in chronological order, mixing channels as requested.
    /// `None` averages all channels; `Some(idx)` returns channel `idx` only.
    pub fn read_latest(&self, n_frames: usize, channel: Option<usize>) -> Option<Vec<PrcFmt>> {
        if self.channels.is_empty() {
            return None;
        }
        let available = self.total_written.min(RING_BUFFER_CAPACITY);
        if available < n_frames {
            return None;
        }
        let start = (self.write_pos + RING_BUFFER_CAPACITY - n_frames) % RING_BUFFER_CAPACITY;
        let result = match channel {
            Some(ch_idx) => (0..n_frames)
                .map(|i| self.channels[ch_idx][(start + i) % RING_BUFFER_CAPACITY])
                .collect(),
            None => {
                let n = self.channels.len() as PrcFmt;
                (0..n_frames)
                    .map(|i| {
                        let idx = (start + i) % RING_BUFFER_CAPACITY;
                        self.channels.iter().map(|ch| ch[idx]).sum::<PrcFmt>() / n
                    })
                    .collect()
            }
        };
        Some(result)
    }
}

impl Default for AudioRingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// --- Window cache ---

static WINDOW_CACHE: LazyLock<Mutex<HashMap<usize, Arc<[PrcFmt]>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static FFT_PLANNER: LazyLock<Mutex<RealFftPlanner<PrcFmt>>> =
    LazyLock::new(|| Mutex::new(RealFftPlanner::new()));

fn get_hann_window(n: usize) -> Arc<[PrcFmt]> {
    let mut cache = WINDOW_CACHE.lock().unwrap();
    if let Some(w) = cache.get(&n) {
        return Arc::clone(w);
    }
    // Compute in f64 for trig precision, then store as PrcFmt.
    let window: Arc<[PrcFmt]> = (0..n)
        .map(|i| {
            (0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos())) as PrcFmt
        })
        .collect();
    cache.insert(n, Arc::clone(&window));
    window
}

// --- Spectrum computation ---

#[derive(Debug, serde::Serialize, PartialEq)]
pub struct SpectrumData {
    pub frequencies: Arc<[f32]>,
    pub magnitudes: Vec<f32>,
}

static FREQ_CACHE: LazyLock<Mutex<HashMap<(u64, u64, usize), Arc<[f32]>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get_frequencies(min_freq: f64, max_freq: f64, n_bins: usize) -> Arc<[f32]> {
    let key = (min_freq.to_bits(), max_freq.to_bits(), n_bins);
    let mut cache = FREQ_CACHE.lock().unwrap();
    if let Some(f) = cache.get(&key) {
        return Arc::clone(f);
    }
    let log_ratio = (max_freq / min_freq).powf(1.0 / (n_bins - 1) as f64);
    let freqs: Arc<[f32]> = (0..n_bins)
        .map(|i| (min_freq * log_ratio.powi(i as i32)) as f32)
        .collect();
    cache.insert(key, Arc::clone(&freqs));
    freqs
}

/// Smallest power-of-two FFT length that provides at least one bin at `min_freq`.
pub fn fft_length_for(min_freq: f64, samplerate: usize) -> usize {
    let min_len = (samplerate as f64 / min_freq).ceil() as usize;
    min_len.next_power_of_two().min(RING_BUFFER_CAPACITY)
}

/// Compute a spectrum from a pre-copied signal buffer.
///
/// Separated from [`compute_spectrum`] so callers can release any shared-data lock
/// before the FFT runs. `signal.len()` is used as the FFT length.
///
/// Returns `n_bins` log-spaced bins from `min_freq` to `max_freq` (Hz).
/// Magnitudes are in dBFS where 0 dBFS = a full-scale (amplitude 1.0) sine wave.
pub fn compute_spectrum_from_signal(
    signal: Vec<PrcFmt>,
    min_freq: f64,
    max_freq: f64,
    n_bins: usize,
    samplerate: usize,
) -> Result<SpectrumData, String> {
    let fft_len = signal.len();

    // Apply Hann window in-place, reusing the signal allocation.
    let window = get_hann_window(fft_len);
    let window_sum: PrcFmt = window.iter().sum();
    let mut windowed = signal;
    windowed
        .iter_mut()
        .zip(window.iter())
        .for_each(|(s, w)| *s *= w);

    // RealFFT — plan is cached inside the global planner; clone the Arc out before
    // releasing the lock so the (potentially slow) FFT runs without holding it.
    let fft = FFT_PLANNER.lock().unwrap().plan_fft_forward(fft_len);
    let mut spectrum = fft.make_output_vec();
    fft.process(&mut windowed, &mut spectrum)
        .map_err(|e| format!("FFT error: {e}"))?;

    // One-sided amplitude-squared spectrum, normalized so that a full-scale
    // sine (amplitude 1.0) at any bin gives power = 1.0 (0 dBFS).
    //
    //   a[k] = 2 * |X[k]| / window_sum   for 0 < k < N/2  (factor 2: single-sided)
    //   a[k] = |X[k]| / window_sum        for k = 0 (DC) and k = N/2 (Nyquist)
    //   p[k] = a[k]^2
    let n_fft = spectrum.len(); // fft_len / 2 + 1

    // norm_sqr avoids a sqrt that would only be undone by the squaring step.
    // DC and Nyquist use scale 1^2 = 1; all other bins use scale 2^2 = 4 (single-sided).
    let inv_w2 = 1.0 / (window_sum * window_sum);
    let power: Vec<PrcFmt> = spectrum
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let scale: PrcFmt = if k == 0 || k == n_fft - 1 { 1.0 } else { 4.0 };
            scale * c.norm_sqr() * inv_w2
        })
        .collect();

    // Frequency-axis calculations stay in f64 to keep index arithmetic exact.
    let freq_res = samplerate as f64 / fft_len as f64;
    let log_ratio = (max_freq / min_freq).powf(1.0 / (n_bins - 1) as f64);

    let frequencies = get_frequencies(min_freq, max_freq, n_bins);
    let mut magnitudes = Vec::with_capacity(n_bins);

    for i in 0..n_bins {
        let f_center = min_freq * log_ratio.powi(i as i32);
        // Geometric midpoints to the neighboring output bins define this bin's edges.
        let f_low = if i == 0 {
            min_freq
        } else {
            f_center / log_ratio.sqrt()
        };
        let f_high = if i == n_bins - 1 {
            max_freq
        } else {
            f_center * log_ratio.sqrt()
        };

        let k_low = (f_low / freq_res).floor() as usize;
        let k_high = ((f_high / freq_res).ceil() as usize).min(n_fft - 1);

        let peak_power: PrcFmt = if k_low <= k_high {
            power[k_low..=k_high].iter().copied().fold(0.0, PrcFmt::max)
        } else {
            // Frequency range narrower than one FFT bin: use nearest bin.
            let k_nearest = ((f_center / freq_res).round() as usize).min(n_fft - 1);
            power[k_nearest]
        };

        magnitudes.push((10.0 * peak_power.max(1e-30).log10()) as f32);
    }

    Ok(SpectrumData {
        frequencies,
        magnitudes,
    })
}

/// Convenience wrapper: validate parameters, copy from ring buffer, then compute.
/// For callers that already hold a lock, prefer calling [`fft_length_for`],
/// [`AudioRingBuffer::read_latest`], and [`compute_spectrum_from_signal`] directly
/// so the lock can be released before the FFT runs.
pub fn compute_spectrum(
    buffer: &AudioRingBuffer,
    min_freq: f64,
    max_freq: f64,
    n_bins: usize,
    channel: Option<usize>,
    samplerate: usize,
) -> Result<SpectrumData, String> {
    if n_bins < 2 {
        return Err("n_bins must be at least 2".to_string());
    }
    if min_freq <= 0.0 || min_freq >= max_freq {
        return Err("Invalid frequency range: min_freq must be > 0 and < max_freq".to_string());
    }
    if samplerate == 0 {
        return Err("Sample rate not available".to_string());
    }
    let n_ch = buffer.channel_count();
    if n_ch == 0 {
        return Err("No audio data available".to_string());
    }
    if let Some(ch) = channel
        && ch >= n_ch
    {
        return Err(format!(
            "Channel {ch} out of range ({n_ch} channels available)"
        ));
    }

    let fft_len = fft_length_for(min_freq, samplerate);
    let signal = buffer
        .read_latest(fft_len, channel)
        .ok_or("Insufficient data in buffer")?;

    compute_spectrum_from_signal(signal, min_freq, max_freq, n_bins, samplerate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiochunk::AudioChunk;
    use std::time::Instant;

    fn make_chunk_sine(freq: f64, amplitude: f64, samplerate: usize, frames: usize) -> AudioChunk {
        let waveform: Vec<PrcFmt> = (0..frames)
            .map(|n| {
                (amplitude
                    * (2.0 * std::f64::consts::PI * freq * n as f64 / samplerate as f64).sin())
                    as PrcFmt
            })
            .collect();
        AudioChunk {
            frames,
            channels: 1,
            maxval: amplitude as PrcFmt,
            minval: -(amplitude as PrcFmt),
            timestamp: Instant::now(),
            valid_frames: frames,
            waveforms: vec![waveform],
        }
    }

    #[test]
    fn full_scale_sine_is_zero_dbfs() {
        let samplerate = 48000;
        let freq = 1000.0;
        let frames = 16384;
        let chunk = make_chunk_sine(freq, 1.0, samplerate, frames);
        let mut buf = AudioRingBuffer::new();
        buf.push_chunk(&chunk);

        let result = compute_spectrum(&buf, 20.0, 20000.0, 200, None, samplerate).unwrap();

        // Find the bin closest to 1 kHz.
        let peak_db = result
            .frequencies
            .iter()
            .zip(result.magnitudes.iter())
            .min_by(|(fa, _), (fb, _)| {
                (*fa - freq as f32)
                    .abs()
                    .partial_cmp(&(*fb - freq as f32).abs())
                    .unwrap()
            })
            .map(|(_, m)| *m)
            .unwrap();

        // Allow ±3 dB tolerance for windowing and bin edge effects.
        assert!(
            peak_db > -3.0 && peak_db < 1.0,
            "1 kHz full-scale sine peak = {peak_db:.1} dBFS, expected near 0"
        );
    }

    #[test]
    fn insufficient_data_returns_error() {
        let buf = AudioRingBuffer::new();
        assert!(compute_spectrum(&buf, 20.0, 20000.0, 100, None, 48000).is_err());
    }

    #[test]
    fn channel_out_of_range_returns_error() {
        let samplerate = 48000;
        let chunk = make_chunk_sine(1000.0, 0.5, samplerate, 8192);
        let mut buf = AudioRingBuffer::new();
        buf.push_chunk(&chunk);
        assert!(compute_spectrum(&buf, 20.0, 20000.0, 100, Some(5), samplerate).is_err());
    }

    #[test]
    fn ring_buffer_wraps_correctly() {
        let mut buf = AudioRingBuffer::new();
        // Fill more than capacity to exercise wrapping.
        for _ in 0..5 {
            let chunk = make_chunk_sine(440.0, 0.5, 48000, 65536);
            buf.push_chunk(&chunk);
        }
        assert_eq!(buf.channel_count(), 1);
        assert!(buf.read_latest(65536, None).is_some());
    }
}
