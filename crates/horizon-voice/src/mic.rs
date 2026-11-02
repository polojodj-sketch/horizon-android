//! Microphone capture: a cpal input stream resampled to 48 kHz mono f32,
//! delivered in 20 ms frames.

use std::sync::mpsc::Sender;

use crate::protocol::{FRAME_SAMPLES, SAMPLE_RATE};

#[cfg(feature = "mic")]
pub struct MicCapture {
    // Dropping the stream stops capture.
    _stream: cpal::Stream,
}

#[cfg(feature = "mic")]
impl MicCapture {
    /// Starts capturing and sends complete 960-sample frames to `tx`.
    /// Returns `None` when no input device or stream could be opened.
    pub fn start(tx: Sender<Vec<f32>>) -> Option<Self> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = host.default_input_device()?;
        let config = device.default_input_config().ok()?;
        let channels = config.channels().max(1) as usize;
        let in_rate = config.sample_rate().0;
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, channels, in_rate, tx),
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, channels, in_rate, tx),
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, channels, in_rate, tx),
            cpal::SampleFormat::U8 => build::<u8>(&device, &config, channels, in_rate, tx),
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, channels, in_rate, tx),
            cpal::SampleFormat::U32 => build::<u32>(&device, &config, channels, in_rate, tx),
            cpal::SampleFormat::I8 => build::<i8>(&device, &config, channels, in_rate, tx),
            _ => return None,
        }?;
        stream.play().ok()?;
        Some(Self { _stream: stream })
    }
}

#[cfg(feature = "mic")]
fn build<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    channels: usize,
    in_rate: u32,
    tx: Sender<Vec<f32>>,
) -> Option<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    let acc = std::sync::Arc::new(std::sync::Mutex::new(Accumulator::new(in_rate, channels)));
    let moved = acc.clone();
    device
        .build_input_stream(
            &config.clone().into(),
            move |data: &[T], _| {
                let mut acc = moved.lock().unwrap_or_else(|e| e.into_inner());
                acc.push(data, &tx);
            },
            |e| log::warn!("voice input error: {e}"),
            None,
        )
        .map_err(|e| log::warn!("voice input stream: {e}"))
        .ok()
}

/// Converts arbitrary-rate multi-channel input into 48 kHz mono frames,
/// carrying the fractional read position across callbacks.
struct Accumulator {
    /// Input samples per output sample.
    step: f64,
    /// Fractional position in the current callback's frames.
    pos: f64,
    channels: usize,
    out: Vec<f32>,
}

impl Accumulator {
    fn new(in_rate: u32, channels: usize) -> Self {
        Self { step: f64::from(in_rate) / f64::from(SAMPLE_RATE), pos: 0.0, channels, out: Vec::with_capacity(FRAME_SAMPLES) }
    }

    fn push<T>(&mut self, data: &[T], tx: &Sender<Vec<f32>>)
    where
        T: Copy,
        f32: cpal::FromSample<T>,
    {
        let frames = data.len() / self.channels;
        if frames == 0 {
            return;
        }
        while self.pos < frames as f64 {
            let i = (self.pos as usize).min(frames - 1);
            let mut mono = 0.0f32;
            for c in 0..self.channels {
                mono += f32::from_sample(data[i * self.channels + c]);
            }
            mono /= self.channels as f32;
            self.out.push(mono);
            self.pos += self.step;
            if self.out.len() >= FRAME_SAMPLES {
                let frame = std::mem::replace(&mut self.out, Vec::with_capacity(FRAME_SAMPLES));
                if tx.send(frame).is_err() {
                    return;
                }
            }
        }
        self.pos -= frames as f64;
    }
}
