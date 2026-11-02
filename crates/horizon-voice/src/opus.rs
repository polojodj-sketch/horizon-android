//! Opus codec wrappers over the vendored libopus FFI.

use std::os::raw::{c_int, c_uchar};

#[cfg_attr(not(feature = "mic"), allow(dead_code))]
#[repr(C)]
struct OpusEncoder {
    _private: [u8; 0],
}
#[cfg_attr(not(feature = "mic"), allow(dead_code))]
#[repr(C)]
struct OpusDecoder {
    _private: [u8; 0],
}

#[allow(non_camel_case_types, dead_code)]
type opus_int32 = i32;

unsafe extern "C" {
    fn opus_encoder_create(Fs: opus_int32, channels: c_int, application: c_int, error: *mut c_int) -> *mut OpusEncoder;
    fn opus_encoder_destroy(st: *mut OpusEncoder);
    fn opus_encode(
        st: *mut OpusEncoder,
        pcm: *const i16,
        frame_size: c_int,
        data: *mut c_uchar,
        max_data_bytes: opus_int32,
    ) -> opus_int32;
    fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
    fn opus_decoder_create(Fs: opus_int32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    fn opus_decoder_destroy(st: *mut OpusDecoder);
    fn opus_decode(
        st: *mut OpusDecoder,
        data: *const c_uchar,
        len: opus_int32,
        pcm: *mut i16,
        frame_size: c_int,
        decode_fec: c_int,
    ) -> c_int;
}

// OPUS_APPLICATION_VOIP and OPUS_SET_BITRATE_REQUEST from opus_defines.h.
#[cfg_attr(not(feature = "mic"), allow(dead_code))]
const OPUS_APPLICATION_VOIP: c_int = 2048;
#[cfg_attr(not(feature = "mic"), allow(dead_code))]
const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
#[cfg_attr(not(feature = "mic"), allow(dead_code))]
const OPUS_BITRATE_MAX: i32 = -1;



/// 20 ms at 48 kHz.
pub const FRAME_SAMPLES: usize = 960;

#[cfg(feature = "mic")]
pub struct Encoder {
    ptr: *mut OpusEncoder,
}

#[cfg(feature = "mic")]
unsafe impl Send for Encoder {}

#[cfg(feature = "mic")]
impl Encoder {
    pub fn new() -> Option<Self> {
        let mut error = 0;
        let ptr = unsafe { opus_encoder_create(48_000, 1, OPUS_APPLICATION_VOIP, &mut error) };
        if ptr.is_null() {
            log::warn!("voice: opus_encoder_create failed ({error})");
            return None;
        }
        // SVC's default: 128 kbps.
        unsafe { opus_encoder_ctl(ptr, OPUS_SET_BITRATE_REQUEST, 128_000) };
        Some(Self { ptr })
    }

    /// `frame` holds exactly 960 samples (20 ms at 48 kHz).
    pub fn encode(&mut self, frame: &[f32]) -> Option<Vec<u8>> {
        let samples: Vec<i16> = frame.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        if samples.len() != FRAME_SAMPLES {
            return None;
        }
        let mut out = vec![0u8; 1280];
        let len = unsafe {
            opus_encode(self.ptr, samples.as_ptr(), FRAME_SAMPLES as c_int, out.as_mut_ptr(), out.len() as opus_int32)
        };
        if len <= 0 {
            return None;
        }
        out.truncate(len as usize);
        Some(out)
    }
}

#[cfg(feature = "mic")]
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.ptr) };
    }
}

pub struct Decoder {
    ptr: *mut OpusDecoder,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Option<Self> {
        let mut error = 0;
        let ptr = unsafe { opus_decoder_create(48_000, 1, &mut error) };
        if ptr.is_null() {
            log::warn!("voice: opus_decoder_create failed ({error})");
            return None;
        }
        Some(Self { ptr })
    }

    /// Decodes one opus packet into 48 kHz mono f32. Empty input means the
    /// protocol's end-of-transmission marker.
    pub fn decode(&mut self, data: &[u8]) -> Option<Vec<f32>> {
        if data.is_empty() {
            return Some(Vec::new());
        }
        let mut out = vec![0i16; FRAME_SAMPLES * 6];
        let len = unsafe {
            opus_decode(
                self.ptr,
                data.as_ptr(),
                data.len() as opus_int32,
                out.as_mut_ptr(),
                out.len() as c_int,
                0,
            )
        };
        if len < 0 {
            return None;
        }
        Some(out[..len as usize].iter().map(|s| f32::from(*s) / 32767.0).collect())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.ptr) };
    }
}


