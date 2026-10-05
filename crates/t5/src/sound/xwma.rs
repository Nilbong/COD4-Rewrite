//! xWMA: WMA version 2 packets, as XAudio2 plays them. Most of Black Ops'
//! loaded sounds (every gunshot, reload foley, ...) are stored this way.
//!
//! WMA's coefficient coding rests on Microsoft's large huffman tables, so
//! rather than carry those, the packets are fed to Windows' own WMA decoder
//! (the Media Foundation "WMAudio Decoder" transform in `wmadmod.dll`, part
//! of every desktop Windows with the media features). Elsewhere these
//! sounds do not decode.
//!
//! The `snd_asset` does not record the bit rate the decoder needs, but xWMA
//! packets always hold 16384 samples' worth at the stream's bit rate (8 WMA
//! frames of 2048), so it follows from the packet size: 2230 byte packets at
//! 44.1 kHz are 48 kbit/s, 4096 byte packets at 48 kHz are 96 kbit/s (the
//! only two kinds in the multiplayer zones). The packet count is the length
//! of the seek table, which holds the decoded byte count after each packet.
//!
//! Checked against FFmpeg's WMA decoder: the same samples, except that
//! FFmpeg leaves in the decoder's 4096 sample delay, which Windows trims; the
//! Windows output is exactly as long as the seek table says.

use anyhow::Result;

/// The average byte rates (`nAvgBytesPerSec`) xWMA uses.
const BYTE_RATES: [u32; 8] = [2500, 4000, 6000, 8000, 12000, 16000, 20000, 24000];

/// Samples one xWMA packet covers.
const PACKET_SAMPLES: u64 = 16384;

/// The byte rate of a stream from its packet size.
pub fn byte_rate(block_align: u32, sample_rate: u32) -> u32 {
    let estimate = (block_align as u64 * sample_rate as u64 / PACKET_SAMPLES) as i64;
    BYTE_RATES.into_iter().min_by_key(|&r| (r as i64 - estimate).abs()).unwrap()
}

/// Decode xWMA packets (`block_align` bytes each) to interleaved 16-bit
/// samples.
pub fn decode(data: &[u8], sample_rate: u32, channels: u16, block_align: u32) -> Result<Vec<i16>> {
    #[cfg(windows)]
    {
        mf::decode(data, sample_rate, channels, block_align, byte_rate(block_align, sample_rate))
    }
    #[cfg(not(windows))]
    {
        let _ = (data, sample_rate, channels, block_align);
        anyhow::bail!("xWMA decoding needs Windows' WMA decoder")
    }
}

#[cfg(windows)]
mod mf {
    //! Just enough Media Foundation, called through raw COM vtables.

    use anyhow::{Result, bail};
    use std::ffi::c_void;
    use std::ptr::null_mut;

    type Hresult = i32;

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);

    /// `CLSID_CWMADecMediaObject`, the WMA decoder (both a DMO and an MFT).
    const CLSID_WMA_DECODER: Guid = Guid(0x2eeb4adf, 0x4578, 0x4d10, [0xbc, 0xa7, 0xbb, 0x95, 0x5f, 0x56, 0x32, 0x0a]);
    const IID_IMFTRANSFORM: Guid = Guid(0xbf94c121, 0x5b05, 0x4e6f, [0x80, 0x00, 0xba, 0x59, 0x89, 0x61, 0x41, 0x4d]);

    const CLSCTX_INPROC_SERVER: u32 = 1;
    const MF_VERSION: u32 = 0x0002_0070;
    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_FORMAT_WMAUDIO2: u16 = 0x0161;

    const MFT_MESSAGE_COMMAND_DRAIN: u32 = 1;
    const MFT_MESSAGE_NOTIFY_BEGIN_STREAMING: u32 = 0x1000_0000;
    const MFT_MESSAGE_NOTIFY_START_OF_STREAM: u32 = 0x1000_0003;
    const MFT_OUTPUT_STREAM_PROVIDES_SAMPLES: u32 = 0x100;
    const MF_E_TRANSFORM_NEED_MORE_INPUT: Hresult = 0xC00D_6D72_u32 as i32;
    const MF_E_NOTACCEPTING: Hresult = 0xC00D_36B5_u32 as i32;

    // Vtable slots. IUnknown is 0..=2; IMFTransform's methods follow it,
    // IMFSample's follow IMFAttributes' 30.
    const RELEASE: usize = 2;
    const MFT_GET_OUTPUT_STREAM_INFO: usize = 7;
    const MFT_SET_INPUT_TYPE: usize = 15;
    const MFT_SET_OUTPUT_TYPE: usize = 16;
    const MFT_PROCESS_MESSAGE: usize = 23;
    const MFT_PROCESS_INPUT: usize = 24;
    const MFT_PROCESS_OUTPUT: usize = 25;
    const SAMPLE_CONVERT_TO_CONTIGUOUS_BUFFER: usize = 41;
    const SAMPLE_ADD_BUFFER: usize = 42;
    const BUFFER_LOCK: usize = 3;
    const BUFFER_UNLOCK: usize = 4;
    const BUFFER_SET_CURRENT_LENGTH: usize = 6;

    #[link(name = "ole32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> Hresult;
        fn CoCreateInstance(
            clsid: *const Guid,
            outer: *mut c_void,
            context: u32,
            iid: *const Guid,
            out: *mut *mut c_void,
        ) -> Hresult;
    }

    #[link(name = "mfplat", kind = "raw-dylib")]
    unsafe extern "system" {
        fn MFStartup(version: u32, flags: u32) -> Hresult;
        fn MFCreateMediaType(out: *mut *mut c_void) -> Hresult;
        fn MFInitMediaTypeFromWaveFormatEx(media_type: *mut c_void, format: *const u8, size: u32) -> Hresult;
        fn MFCreateSample(out: *mut *mut c_void) -> Hresult;
        fn MFCreateMemoryBuffer(max_length: u32, out: *mut *mut c_void) -> Hresult;
    }

    #[repr(C)]
    struct OutputStreamInfo {
        flags: u32,
        size: u32,
        alignment: u32,
    }

    #[repr(C)]
    struct OutputDataBuffer {
        stream_id: u32,
        sample: *mut c_void,
        status: u32,
        events: *mut c_void,
    }

    /// An owned COM interface pointer.
    struct Com(*mut c_void);

    impl Com {
        /// Vtable entry `slot`, as a function of type `F`.
        unsafe fn method<F: Copy>(&self, slot: usize) -> F {
            unsafe {
                let vtable = *(self.0 as *const *const usize);
                std::mem::transmute_copy(&*vtable.add(slot))
            }
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { self.method::<unsafe extern "system" fn(*mut c_void) -> u32>(RELEASE)(self.0) };
            }
        }
    }

    fn check(hr: Hresult, what: &str) -> Result<()> {
        if hr < 0 {
            bail!("{what} failed ({:#010x})", hr as u32);
        }
        Ok(())
    }

    /// `WAVEFORMATEX` plus its extra bytes.
    fn wave_format(tag: u16, channels: u16, rate: u32, byte_rate: u32, align: u16, extra: &[u8]) -> Vec<u8> {
        let mut f = Vec::with_capacity(18 + extra.len());
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&byte_rate.to_le_bytes());
        f.extend_from_slice(&align.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        f.extend_from_slice(extra);
        f
    }

    fn media_type(format: &[u8]) -> Result<Com> {
        unsafe {
            let mut mt = null_mut();
            check(MFCreateMediaType(&mut mt), "MFCreateMediaType")?;
            let mt = Com(mt);
            check(
                MFInitMediaTypeFromWaveFormatEx(mt.0, format.as_ptr(), format.len() as u32),
                "MFInitMediaTypeFromWaveFormatEx",
            )?;
            Ok(mt)
        }
    }

    /// A sample holding one buffer of `capacity` bytes, `data` copied in.
    fn sample(capacity: u32, data: &[u8]) -> Result<Com> {
        unsafe {
            let (mut s, mut b) = (null_mut(), null_mut());
            check(MFCreateSample(&mut s), "MFCreateSample")?;
            let s = Com(s);
            check(MFCreateMemoryBuffer(capacity.max(data.len() as u32), &mut b), "MFCreateMemoryBuffer")?;
            let b = Com(b);
            if !data.is_empty() {
                let mut p: *mut u8 = null_mut();
                let lock: unsafe extern "system" fn(*mut c_void, *mut *mut u8, *mut u32, *mut u32) -> Hresult =
                    b.method(BUFFER_LOCK);
                check(lock(b.0, &mut p, null_mut(), null_mut()), "IMFMediaBuffer::Lock")?;
                std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
                b.method::<unsafe extern "system" fn(*mut c_void) -> Hresult>(BUFFER_UNLOCK)(b.0);
            }
            let set_len: unsafe extern "system" fn(*mut c_void, u32) -> Hresult = b.method(BUFFER_SET_CURRENT_LENGTH);
            check(set_len(b.0, data.len() as u32), "IMFMediaBuffer::SetCurrentLength")?;
            let add: unsafe extern "system" fn(*mut c_void, *mut c_void) -> Hresult = s.method(SAMPLE_ADD_BUFFER);
            check(add(s.0, b.0), "IMFSample::AddBuffer")?;
            Ok(s)
        }
    }

    /// Append the bytes of a sample's buffers.
    fn read_sample(s: &Com, out: &mut Vec<u8>) -> Result<()> {
        unsafe {
            let mut b = null_mut();
            let contiguous: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Hresult =
                s.method(SAMPLE_CONVERT_TO_CONTIGUOUS_BUFFER);
            check(contiguous(s.0, &mut b), "IMFSample::ConvertToContiguousBuffer")?;
            let b = Com(b);
            let (mut p, mut len): (*mut u8, u32) = (null_mut(), 0);
            let lock: unsafe extern "system" fn(*mut c_void, *mut *mut u8, *mut u32, *mut u32) -> Hresult =
                b.method(BUFFER_LOCK);
            check(lock(b.0, &mut p, null_mut(), &mut len), "IMFMediaBuffer::Lock")?;
            out.extend_from_slice(std::slice::from_raw_parts(p, len as usize));
            b.method::<unsafe extern "system" fn(*mut c_void) -> Hresult>(BUFFER_UNLOCK)(b.0);
            Ok(())
        }
    }

    fn init() -> Result<()> {
        static STARTED: std::sync::OnceLock<Hresult> = std::sync::OnceLock::new();
        // Any apartment will do (the decoder is free-threaded), so a thread
        // already initialised differently is fine.
        unsafe { CoInitializeEx(null_mut(), 0) };
        check(*STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, 0) }), "MFStartup")
    }

    pub fn decode(data: &[u8], rate: u32, channels: u16, block_align: u32, byte_rate: u32) -> Result<Vec<i16>> {
        init()?;
        unsafe {
            let mut t = null_mut();
            check(
                CoCreateInstance(&CLSID_WMA_DECODER, null_mut(), CLSCTX_INPROC_SERVER, &IID_IMFTRANSFORM, &mut t),
                "creating the WMA decoder",
            )?;
            let t = Com(t);

            // WMA v2's extra format bytes: samples per block, encode options
            // (exponent VLC, bit reservoir, variable block sizes: what xWMA
            // always uses) and super block alignment.
            let mut extra = Vec::new();
            extra.extend_from_slice(&2048u32.to_le_bytes());
            extra.extend_from_slice(&0x1fu16.to_le_bytes());
            extra.extend_from_slice(&0u32.to_le_bytes());
            let input =
                media_type(&wave_format(WAVE_FORMAT_WMAUDIO2, channels, rate, byte_rate, block_align as u16, &extra))?;
            let pcm_align = channels * 2;
            let output =
                media_type(&wave_format(WAVE_FORMAT_PCM, channels, rate, rate * pcm_align as u32, pcm_align, &[]))?;
            let set_type: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> Hresult =
                t.method(MFT_SET_INPUT_TYPE);
            check(set_type(t.0, 0, input.0, 0), "setting the xWMA input type")?;
            let set_type: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> Hresult =
                t.method(MFT_SET_OUTPUT_TYPE);
            check(set_type(t.0, 0, output.0, 0), "setting the PCM output type")?;

            let mut info = OutputStreamInfo { flags: 0, size: 0, alignment: 0 };
            let get_info: unsafe extern "system" fn(*mut c_void, u32, *mut OutputStreamInfo) -> Hresult =
                t.method(MFT_GET_OUTPUT_STREAM_INFO);
            check(get_info(t.0, 0, &mut info), "IMFTransform::GetOutputStreamInfo")?;
            let provides = info.flags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES != 0;
            // A packet's worth of PCM at least.
            let out_size = info.size.max(super::PACKET_SAMPLES as u32 * pcm_align as u32);

            let message: unsafe extern "system" fn(*mut c_void, u32, usize) -> Hresult = t.method(MFT_PROCESS_MESSAGE);
            message(t.0, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            message(t.0, MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            let process_input: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, u32) -> Hresult =
                t.method(MFT_PROCESS_INPUT);
            let process_output: unsafe extern "system" fn(
                *mut c_void,
                u32,
                u32,
                *mut OutputDataBuffer,
                *mut u32,
            ) -> Hresult = t.method(MFT_PROCESS_OUTPUT);

            let mut pcm = Vec::new();
            let drain = |pcm: &mut Vec<u8>| -> Result<()> {
                loop {
                    let own = if provides { None } else { Some(sample(out_size, &[])?) };
                    let mut buf = OutputDataBuffer {
                        stream_id: 0,
                        sample: own.as_ref().map_or(null_mut(), |s| s.0),
                        status: 0,
                        events: null_mut(),
                    };
                    let mut status = 0;
                    let hr = process_output(t.0, 0, 1, &mut buf, &mut status);
                    if !buf.events.is_null() {
                        drop(Com(buf.events));
                    }
                    if hr == MF_E_TRANSFORM_NEED_MORE_INPUT {
                        return Ok(());
                    }
                    check(hr, "IMFTransform::ProcessOutput")?;
                    match own {
                        Some(s) => read_sample(&s, pcm)?,
                        None => read_sample(&Com(buf.sample), pcm)?,
                    }
                }
            };
            for packet in data.chunks(block_align as usize) {
                let s = sample(block_align, packet)?;
                loop {
                    let hr = process_input(t.0, 0, s.0, 0);
                    if hr == MF_E_NOTACCEPTING {
                        drain(&mut pcm)?;
                        continue;
                    }
                    check(hr, "IMFTransform::ProcessInput")?;
                    break;
                }
                drain(&mut pcm)?;
            }
            message(t.0, MFT_MESSAGE_COMMAND_DRAIN, 0);
            drain(&mut pcm)?;
            Ok(pcm.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
        }
    }
}
