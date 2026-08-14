//! System audio capture via a Core Audio process tap, resampled to 16 kHz
//! mono. macOS-only (see `#[cfg]` on the module declaration in `lib.rs`).
//!
//! Sequence (mirrors the anarlog reference implementation, which ships this
//! exact flow against cidre 0.15 in production): create a mono global tap
//! excluding no processes, wrap it in a private aggregate device, attach an
//! IOProc to that aggregate device, and start it. Unlike anarlog we don't
//! need a lock-free ring buffer + waker: chunks go straight over an
//! `std::sync::mpsc::Sender`, same as `mic.rs`.
//!
//! Known limitation (acceptable for MVP-0): if the output device's sample
//! rate changes mid-capture (e.g. the user switches audio output), we keep
//! resampling from the rate read once at setup. Anarlog re-probes the tap's
//! ASBD periodically and adjusts; we don't.

use crate::{Channel, Chunk, resample_to_16k};
use anyhow::{Context, Result};
use cidre::{av, cat, cf, core_audio as ca, ns, os};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

/// Name of the private aggregate device we create. `mic.rs` checks for this
/// substring to avoid ever binding to our own tap as a "default input".
const TAP_DEVICE_NAME: &str = "simstim-tap";

/// State the IOProc callback needs. Lives in a `Box` so it has a stable
/// address across calls; the callback receives a raw pointer to it via
/// Core Audio's `client_data`.
struct Ctx {
    tx: Sender<Chunk>,
    rate: u32,
    common_format: av::AudioCommonFormat,
    carry: Vec<f32>,
}

/// RAII handle for the running tap + aggregate device. Field order is the
/// teardown order (Rust drops struct fields top to bottom): `started` first,
/// which calls `AudioDeviceStop` and detaches the IOProc so Core Audio's
/// callback thread can no longer touch `ctx`; only then is `ctx` freed;
/// `tap` (which destroys the process tap) goes last. This mirrors anarlog's
/// `SpeakerStream` field-drop-order pattern instead of leaking the context
/// and freeing it manually.
pub struct SpeakerHandle {
    _started: ca::hardware::StartedDevice<ca::AggregateDevice>,
    _ctx: Box<Ctx>,
    _tap: ca::TapGuard,
}

/// Starts capturing system audio output via a Core Audio process tap and
/// sends resampled 16 kHz mono [`Chunk`]s (tagged [`Channel::Them`]) to `tx`.
///
/// Triggers the macOS "System Audio Recording" permission prompt on first
/// call.
pub fn start_speaker(tx: Sender<Chunk>) -> Result<SpeakerHandle> {
    let tap_desc = ca::TapDesc::with_mono_global_tap_excluding_processes(&ns::Array::new());
    let tap = tap_desc
        .create_process_tap()
        .context("create process tap (needs System Audio Recording permission)")?;

    let asbd = tap.asbd().context("read tap format")?;
    let rate = asbd.sample_rate as u32;
    let common_format = av::AudioFormat::with_asbd(&asbd)
        .context("unsupported system audio format")?
        .common_format();
    let tap_uid = tap.uid().context("read tap uid")?;

    let sub_tap = cf::DictionaryOf::with_keys_values(
        &[ca::sub_device_keys::uid()],
        &[tap_uid.as_type_ref()],
    );
    let agg_desc = cf::DictionaryOf::with_keys_values(
        &[
            ca::aggregate_device_keys::is_private(),
            ca::aggregate_device_keys::tap_auto_start(),
            ca::aggregate_device_keys::name(),
            ca::aggregate_device_keys::uid(),
            ca::aggregate_device_keys::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            // `tap_auto_start = true` means AudioDeviceStart blocks until a
            // tapped process starts *receiving* audio, which for our
            // exclude-nothing global tap can be forever: `device_start`
            // below is synchronous, so that would hang `start_speaker`
            // until some app happens to play something. We want the device
            // running immediately and simply deliver silence until there's
            // real audio, so this must be false (anarlog: speaker/macos.rs
            // sets the same key to false for the same reason).
            cf::Boolean::value_false().as_type_ref(),
            cf::String::from_str(TAP_DEVICE_NAME).as_ref(),
            cf::Uuid::new().to_cf_string().as_ref(),
            cf::ArrayOf::from_slice(&[sub_tap.as_ref()]).as_ref(),
        ],
    );

    let agg_device = ca::AggregateDevice::with_desc(&agg_desc).context("create aggregate device")?;

    let mut ctx = Box::new(Ctx {
        tx,
        rate,
        common_format,
        carry: Vec::new(),
    });
    // Known MVP-0 gap (mirrors anarlog): if `create_io_proc_id` succeeds but
    // `device_start` below fails, `proc_id` is never destroyed via
    // `AudioDeviceDestroyIOProcID`. Acceptable here since a failed
    // `start_speaker` call means the caller won't get a `SpeakerHandle` to
    // retry with anyway, and `agg_device` itself is dropped without ever
    // having lived in a `StartedDevice`, so it's collected fine on the next
    // launch.
    let proc_id = agg_device
        .create_io_proc_id(io_proc, Some(ctx.as_mut()))
        .context("create IO proc")?;
    let started = ca::device_start(agg_device, Some(proc_id)).context("start aggregate device")?;

    Ok(SpeakerHandle {
        _started: started,
        _ctx: ctx,
        _tap: tap,
    })
}

/// Core Audio IOProc: called on Core Audio's own real-time thread, not one we
/// spawned. Must stay allocation-light; the mpsc `send` of a small `Vec` is
/// the one allocation-per-callback we accept for MVP-0 (same tradeoff
/// `mic.rs` makes for its mono mixdown).
extern "C" fn io_proc(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input_data: &cat::AudioBufList<1>,
    _input_time: &cat::AudioTimeStamp,
    _output_data: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    ctx: Option<&mut Ctx>,
) -> os::Status {
    let Some(ctx) = ctx else {
        return os::Status::NO_ERR;
    };

    let buf = &input_data.buffers[0];
    if buf.data.is_null() || buf.data_bytes_size == 0 {
        return os::Status::NO_ERR;
    }

    let samples: Vec<f32> = match ctx.common_format {
        av::AudioCommonFormat::PcmF32 => match read_samples::<f32>(buf) {
            Some(s) => s.to_vec(),
            None => {
                log_format_issue_once("buffer unreadable (null/misaligned) for PcmF32");
                return os::Status::NO_ERR;
            }
        },
        av::AudioCommonFormat::PcmF64 => match read_samples::<f64>(buf) {
            Some(s) => s.iter().map(|&v| v as f32).collect(),
            None => {
                log_format_issue_once("buffer unreadable (null/misaligned) for PcmF64");
                return os::Status::NO_ERR;
            }
        },
        av::AudioCommonFormat::PcmI32 => match read_samples::<i32>(buf) {
            Some(s) => s.iter().map(|&v| pcm_i32_to_f32(v)).collect(),
            None => {
                log_format_issue_once("buffer unreadable (null/misaligned) for PcmI32");
                return os::Status::NO_ERR;
            }
        },
        av::AudioCommonFormat::PcmI16 => match read_samples::<i16>(buf) {
            Some(s) => s.iter().map(|&v| pcm_i16_to_f32(v)).collect(),
            None => {
                log_format_issue_once("buffer unreadable (null/misaligned) for PcmI16");
                return os::Status::NO_ERR;
            }
        },
        _ => {
            log_format_issue_once("unrecognized common format, dropping audio");
            return os::Status::NO_ERR;
        }
    };

    let resampled = resample_to_16k(&samples, ctx.rate, &mut ctx.carry);
    let _ = ctx.tx.send(Chunk {
        channel: Channel::Them,
        samples: resampled,
    });

    os::Status::NO_ERR
}

/// Set once the first format/alignment problem has been logged, so a
/// misbehaving tap prints one diagnostic line instead of flooding stderr
/// from the audio callback (and so a real format problem is distinguishable
/// from plain silence, which produces valid, merely-quiet chunks).
static FORMAT_ISSUE_LOGGED: AtomicBool = AtomicBool::new(false);

fn log_format_issue_once(msg: &str) {
    if FORMAT_ISSUE_LOGGED
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        eprintln!("simstim-capture: speaker tap {msg}");
    }
}

/// Reinterprets a raw `AudioBuf`'s bytes as a `[T]`, refusing to do so if the
/// pointer is null, empty, or misaligned for `T`.
fn read_samples<T: Copy>(buf: &cat::AudioBuf) -> Option<&[T]> {
    let byte_count = buf.data_bytes_size as usize;
    if byte_count == 0 || buf.data.is_null() {
        return None;
    }
    let data = buf.data as *const T;
    if !(data as usize).is_multiple_of(std::mem::align_of::<T>()) {
        return None;
    }
    let sample_count = byte_count / std::mem::size_of::<T>();
    if sample_count == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, sample_count) })
}

/// Standard PCM16 -> f32 normalization (symmetric around 0, `i16::MIN` maps
/// to exactly -1.0 rather than overshooting).
fn pcm_i16_to_f32(sample: i16) -> f32 {
    const SCALE: f32 = i16::MAX as f32;
    if sample == i16::MIN {
        -1.0
    } else {
        sample as f32 / SCALE
    }
}

/// Standard PCM32 -> f32 normalization, same convention as [`pcm_i16_to_f32`].
fn pcm_i32_to_f32(sample: i32) -> f32 {
    const SCALE: f32 = i32::MAX as f32;
    if sample == i32::MIN {
        -1.0
    } else {
        sample as f32 / SCALE
    }
}
