//! MP3 bytes in, 48 kHz interleaved stereo `f32` out: `nanomp3-core` decodes frame by
//! frame, mono is doubled, and a linear resampler takes the stream's rate (44.1 kHz for
//! most stations) to the mixer's. Linear is plenty for a car radio, and it is what the
//! web build's worklet does.

use nanomp3_core::{Channels, DecodeError, Decoder, MAX_SAMPLES_PER_FRAME};

/// Output rate, Hz (the mixer's).
pub const RATE: u32 = audio::mixer::SAMPLE_RATE;

/// Input kept ahead of the decoder: frames only count once the frames after them are
/// seen (the decoder's docs ask for several frames; an MP3 frame is at most ~1.4 KiB).
const LOOKAHEAD: usize = 8 * 1024;

pub struct Mp3 {
    decoder: Box<Decoder>,
    input: Vec<u8>,
    pcm: Vec<f32>,
    resampler: Resampler,
    /// Frames decoded so far.
    pub frames: u64,
}

impl Default for Mp3 {
    fn default() -> Mp3 {
        Mp3::new()
    }
}

impl Mp3 {
    pub fn new() -> Mp3 {
        Mp3 {
            decoder: Box::new(Decoder::new()),
            input: Vec::with_capacity(4 * LOOKAHEAD),
            pcm: vec![0.0; MAX_SAMPLES_PER_FRAME],
            resampler: Resampler::default(),
            frames: 0,
        }
    }

    /// Takes the next bytes of the stream and appends what they decode to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<f32>) {
        self.input.extend_from_slice(bytes);
        self.decode(LOOKAHEAD, out);
    }

    /// Decodes what is left (a file ended).
    pub fn finish(&mut self, out: &mut Vec<f32>) {
        self.decode(0, out);
    }

    fn decode(&mut self, keep: usize, out: &mut Vec<f32>) {
        let mut at = 0;
        while self.input.len() - at > keep {
            let (used, result) = self.decoder.decode(&self.input[at..], &mut self.pcm);
            at += used;
            match result {
                Ok(info) => {
                    let n = info.samples_produced * usize::from(info.channels.num());
                    let stereo = info.channels == Channels::Stereo;
                    self.resampler
                        .push(&self.pcm[..n], stereo, info.sample_rate, out);
                    self.frames += 1;
                }
                // Starting mid-stream, the first frames lack their reservoir; corrupt
                // frames are skipped.
                Err(DecodeError::ReservoirUnavailable(_) | DecodeError::Corrupt(_)) => {}
                Err(_) if used == 0 => break,
                Err(_) => {}
            }
            if used == 0 {
                break;
            }
        }
        self.input.drain(..at);
    }
}

/// Linear interpolation from any rate to [`RATE`], carrying its phase across calls.
#[derive(Default)]
pub struct Resampler {
    rate: u32,
    /// Position of the next output sample between `prev` (0) and the next input (1).
    frac: f64,
    prev: [f32; 2],
}

impl Resampler {
    /// Appends `input` (interleaved stereo, or mono when `!stereo`) at `rate` Hz to `out`
    /// as stereo at [`RATE`].
    pub fn push(&mut self, input: &[f32], stereo: bool, rate: u32, out: &mut Vec<f32>) {
        if rate == 0 {
            return;
        }
        if rate != self.rate {
            self.rate = rate;
            self.frac = 0.0;
        }
        let step = f64::from(rate) / f64::from(RATE);
        let channels = if stereo { 2 } else { 1 };
        for frame in input.chunks_exact(channels) {
            let cur = [frame[0], frame[channels - 1]];
            while self.frac < 1.0 {
                let t = self.frac as f32;
                out.push(self.prev[0] + (cur[0] - self.prev[0]) * t);
                out.push(self.prev[1] + (cur[1] - self.prev[1]) * t);
                self.frac += step;
            }
            self.frac -= 1.0;
            self.prev = cur;
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// One MPEG-1 Layer III frame (128 kb/s, 44.1 kHz, stereo) of digital silence: a
    /// header and all-zero side information and main data. 417 bytes.
    pub fn silent_frame() -> Vec<u8> {
        let mut f = vec![0u8; 417];
        f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x04]);
        f
    }

    #[test]
    fn decodes_generated_frames_at_48k() {
        let stream: Vec<u8> = (0..40).flat_map(|_| silent_frame()).collect();
        let mut mp3 = Mp3::new();
        let mut out = Vec::new();
        // In uneven pieces, like a socket delivers them.
        for piece in stream.chunks(333) {
            mp3.feed(piece, &mut out);
        }
        // Junk after the end lets the decoder see the last frames through.
        mp3.feed(&[0; LOOKAHEAD + 1], &mut out);
        assert!(mp3.frames >= 38, "decoded {} frames", mp3.frames);
        // 1152 samples a frame at 44.1 kHz is ~1254 frames at 48 kHz, stereo.
        let want = mp3.frames as f64 * 1152.0 * 48_000.0 / 44_100.0 * 2.0;
        assert!(
            (out.len() as f64 - want).abs() < 8.0,
            "{} vs {want}",
            out.len()
        );
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn junk_decodes_to_nothing() {
        let mut mp3 = Mp3::new();
        let mut out = Vec::new();
        mp3.feed(&[0x55; 20_000], &mut out);
        assert!(out.is_empty());
        assert_eq!(mp3.frames, 0);
    }

    #[test]
    fn resampling_keeps_rate_phase_and_shape() {
        // A 441 Hz ramp-free check: 44.1 kHz in → 48 kHz out, length and continuity.
        let mut r = Resampler::default();
        let mut out = Vec::new();
        let input: Vec<f32> = (0..44_100).flat_map(|i| [i as f32, -(i as f32)]).collect();
        for part in input.chunks(2 * 1152) {
            r.push(part, true, 44_100, &mut out);
        }
        assert!((out.len() as i64 - 96_000).abs() <= 2, "{}", out.len());
        // A ramp stays a ramp: steps of 44.1/48 input samples, left and right mirrored.
        for w in out.chunks_exact(2).skip(2).collect::<Vec<_>>().windows(2) {
            let d = w[1][0] - w[0][0];
            assert!((d - 0.91875).abs() < 1e-2, "{d}");
            assert_eq!(w[1][0], -w[1][1]);
        }
        // Mono doubles; 48 kHz passes through (one sample late).
        let mut r = Resampler::default();
        let mut out = Vec::new();
        r.push(&[0.5, 0.25], false, 48_000, &mut out);
        assert_eq!(out, [0.0, 0.0, 0.5, 0.5]);
    }
}
