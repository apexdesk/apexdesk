//! Sound through the system's output (cpal: WASAPI, CoreAudio or ALSA).
//! A player's decoding thread fills a [`Ring`] with stereo samples at the
//! device's rate; the device's callback takes them, at the volume set,
//! and gives silence while paused or when they run short.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[cfg(not(target_arch = "wasm32"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Samples waiting to be heard, shared with the device's callback.
#[derive(Default)]
pub struct Ring {
    buf: Mutex<VecDeque<f32>>,
    /// The volume as f32 bits (0 until set).
    volume: AtomicU32,
    pub paused: AtomicBool,
}

impl Ring {
    pub fn new(volume: f32) -> Arc<Ring> {
        let r = Ring::default();
        r.set_volume(volume);
        Arc::new(r)
    }

    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    /// Stereo samples waiting (pairs).
    pub fn frames(&self) -> usize {
        self.buf.lock().unwrap().len() / 2
    }

    pub fn push(&self, stereo: &[f32]) {
        self.buf.lock().unwrap().extend(stereo);
    }

    /// Fills `out` (`channels` interleaved) with what's waiting: left and
    /// right to the first two channels, their mean to a single one.
    pub fn fill(&self, out: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        out.fill(0.0);
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        let vol = self.volume();
        let mut buf = self.buf.lock().unwrap();
        for f in out.chunks_mut(channels) {
            let (Some(l), Some(r)) = (buf.pop_front(), buf.pop_front()) else { break };
            if channels == 1 {
                f[0] = (l + r) * 0.5 * vol;
            } else {
                f[0] = l * vol;
                f[1] = r * vol;
            }
        }
    }
}

/// The open output: sound stops when it's dropped.
pub struct Output {
    #[cfg(not(target_arch = "wasm32"))]
    _stream: cpal::Stream,
    /// Samples per second the device takes.
    pub rate: u32,
}

/// The browser version plays no sound of presentations.
#[cfg(target_arch = "wasm32")]
pub fn open(_ring: Arc<Ring>) -> Result<Output, String> {
    Err("no sound output in the browser".into())
}

/// Opens the default output device, taking its sound from `ring`.
#[cfg(not(target_arch = "wasm32"))]
pub fn open(ring: Arc<Ring>) -> Result<Output, String> {
    let dev = cpal::default_host().default_output_device().ok_or("no sound output")?;
    let sup = dev.default_output_config().map_err(|e| e.to_string())?;
    let format = sup.sample_format();
    let config: cpal::StreamConfig = sup.into();
    let rate = config.sample_rate;
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&dev, config, ring),
        cpal::SampleFormat::I16 => build::<i16>(&dev, config, ring),
        cpal::SampleFormat::U16 => build::<u16>(&dev, config, ring),
        cpal::SampleFormat::I32 => build::<i32>(&dev, config, ring),
        f => return Err(format!("sound output format {f:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Output { _stream: stream, rate })
}

#[cfg(not(target_arch = "wasm32"))]
fn build<T>(dev: &cpal::Device, config: cpal::StreamConfig, ring: Arc<Ring>) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut tmp: Vec<f32> = Vec::new();
    dev.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], _| {
            tmp.resize(out.len(), 0.0);
            ring.fill(&mut tmp, channels);
            for (o, s) in out.iter_mut().zip(&tmp) {
                *o = T::from_sample(*s);
            }
        },
        |e| crate::diag::warning(&format!("sound output: {e}")),
        None,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_fills_channels_at_volume() {
        let r = Ring::new(0.5);
        r.push(&[1.0, -1.0, 0.5, 0.5]);
        let mut out = [9.0; 6];
        r.fill(&mut out, 3);
        assert_eq!(out, [0.5, -0.5, 0.0, 0.25, 0.25, 0.0]);
        r.push(&[1.0, 0.0]);
        let mut mono = [9.0; 2];
        r.fill(&mut mono, 1);
        assert_eq!(mono, [0.25, 0.0]);
        r.push(&[1.0, 1.0]);
        r.paused.store(true, Ordering::Relaxed);
        r.fill(&mut mono, 1);
        assert_eq!(mono, [0.0, 0.0]);
        assert_eq!(r.frames(), 1);
    }
}
