//! Notification sounds. The soft tones are synthesized once at startup: short,
//! quiet, marimba-like two-note chimes that stay pleasant on the hundredth
//! hearing, unlike the Windows "asterisk" sound.

use std::sync::OnceLock;

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

use crate::settings::Sound;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// Rising fourth: something finished.
    Done,
    /// Two soft taps on the same note: something waits for you.
    Waiting,
    /// Falling third: something failed.
    Error,
}

const RATE: u32 = 44_100;
/// Peak amplitude, well below full scale so the chime sits under other audio.
const VOLUME: f32 = 0.16;

fn notes(tone: Tone) -> &'static [(f32, f32)] {
    // (frequency Hz, start s)
    match tone {
        Tone::Done => &[(783.99, 0.0), (1046.50, 0.11)],
        Tone::Waiting => &[(880.00, 0.0), (880.00, 0.16)],
        Tone::Error => &[(659.25, 0.0), (523.25, 0.14)],
    }
}

fn synth(tone: Tone) -> Vec<u8> {
    let len = 0.6;
    let n = (len * RATE as f32) as usize;
    let mut samples = vec![0f32; n];
    for &(freq, start) in notes(tone) {
        let first = (start * RATE as f32) as usize;
        for (i, s) in samples.iter_mut().enumerate().skip(first) {
            let t = (i - first) as f32 / RATE as f32;
            // 5 ms attack, then an exponential ring-out.
            let env = (t / 0.005).min(1.0) * (-t / 0.11).exp();
            let w = std::f32::consts::TAU * freq * t;
            *s += env * (w.sin() + 0.12 * (2.0 * w).sin() + 0.04 * (3.0 * w).sin());
        }
    }
    // Fade out the tail so the buffer never ends on a click.
    let fade = (0.05 * RATE as f32) as usize;
    for i in 0..fade {
        samples[n - 1 - i] *= i as f32 / fade as f32;
    }

    let data_len = (n * 2) as u32;
    let mut wav = Vec::with_capacity(44 + n * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s * VOLUME).clamp(-1.0, 1.0);
        wav.extend_from_slice(&((v * i16::MAX as f32) as i16).to_le_bytes());
    }
    wav
}

/// The WAV buffers must outlive the asynchronous playback, hence static.
fn wavs() -> &'static [Vec<u8>; 3] {
    static WAVS: OnceLock<[Vec<u8>; 3]> = OnceLock::new();
    WAVS.get_or_init(|| [synth(Tone::Done), synth(Tone::Waiting), synth(Tone::Error)])
}

pub fn play(sound: Sound, tone: Tone) {
    match sound {
        Sound::Off => {}
        Sound::System => claudehud::win::beep(),
        Sound::Soft => {
            let wav = &wavs()[tone as usize];
            unsafe { PlaySoundW(wav.as_ptr() as *const u16, std::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
        }
    }
}
