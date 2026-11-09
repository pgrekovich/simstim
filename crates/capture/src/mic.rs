//! Microphone capture: the call app's input device or the system default, resampled to 16 kHz mono.

use crate::{CaptureHandle, Channel, Chunk, resample_to_16k};
use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc::{Sender, channel};

/// Returns whether macOS has granted microphone access, requesting it when
/// the user has not made a choice yet. This checks authorization itself and
/// does not require the microphone to produce a signal.
#[cfg(target_os = "macos")]
pub fn request_permission() -> Result<bool> {
    use cidre::{av, blocks};

    let media_type = av::MediaType::audio();
    let status = av::CaptureDevice::authorization_status_for_media_type(media_type)
        .map_err(|error| anyhow!("could not read microphone permission: {error:?}"))?;
    match status {
        av::AuthorizationStatus::Authorized => Ok(true),
        av::AuthorizationStatus::Denied | av::AuthorizationStatus::Restricted => Ok(false),
        av::AuthorizationStatus::NotDetermined => {
            let (tx, rx) = channel();
            let mut completion = blocks::SendBlock::new1(move |granted: bool| {
                let _ = tx.send(granted);
            });
            av::CaptureDevice::request_access_for_media_type_ch(media_type, &mut completion)
                .map_err(|error| anyhow!("could not request microphone permission: {error:?}"))?;
            rx.recv()
                .map_err(|error| anyhow!("microphone permission request was cancelled: {error}"))
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn request_permission() -> Result<bool> {
    Ok(true)
}

/// Picks the input device to record from.
///
/// `preferred_uid` is a Core Audio device UID (cpal's device id on macOS),
/// normally the input the active call app is capturing with. Call apps choose
/// their microphone independently of the macOS default input, and recording
/// the wrong one yields a silent Me channel. When no UID is given, or the
/// device has gone away since it was looked up, the system default input is
/// used instead so a stale UID can never block a recording.
fn pick_input_device(host: &cpal::Host, preferred_uid: Option<&str>) -> Result<cpal::Device> {
    if let Some(uid) = preferred_uid {
        let preferred = host
            .input_devices()?
            .find(|device| device.id().is_ok_and(|id| id.id() == uid));
        match preferred {
            Some(device) => return Ok(device),
            None => tracing::warn!("preferred input {uid} is not available; using system default"),
        }
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no input device"))
}

/// Starts capturing from an input device and sends resampled 16 kHz mono
/// [`Chunk`]s (tagged [`Channel::Me`]) to `tx`. See [`pick_input_device`]
/// for how `preferred_uid` is used.
///
/// The stream lives on a dedicated thread; the returned [`CaptureHandle`]
/// tears it down on drop by signalling that thread over an mpsc channel.
pub fn start_mic(tx: Sender<Chunk>, preferred_uid: Option<&str>) -> Result<CaptureHandle> {
    let (drop_tx, drop_rx) = channel::<()>();
    let (ready_tx, ready_rx) = channel::<Result<String>>();
    let preferred_uid = preferred_uid.map(str::to_owned);

    std::thread::spawn(move || {
        let build = || -> Result<(cpal::Stream, String)> {
            let host = cpal::default_host();
            let device = pick_input_device(&host, preferred_uid.as_deref())?;
            let name = device
                .description()
                .map(|desc| desc.name().to_string())
                .unwrap_or_else(|_| "unknown input".to_string());
            // Exclude the system-audio tap's aggregate device from microphone selection.
            if crate::is_own_tap(&name) {
                return Err(anyhow!("selected input is our tap"));
            }
            let config = device.default_input_config()?;
            let rate = config.sample_rate();
            let channels = config.channels() as usize;
            let mut carry = Vec::new();
            let stream = device.build_input_stream(
                config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono: Vec<f32> = data
                        .chunks(channels)
                        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                        .collect();
                    let samples = resample_to_16k(&mono, rate, &mut carry);
                    let _ = tx.send(Chunk {
                        channel: Channel::Me,
                        samples,
                    });
                },
                |e| tracing::warn!("mic stream error: {e}"),
                None,
            )?;
            stream.play()?;
            Ok((stream, name))
        };

        match build() {
            Ok((stream, name)) => {
                let _ = ready_tx.send(Ok(name));
                let _ = drop_rx.recv(); // park until the handle is dropped
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    });

    let device_name = ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| anyhow!("mic init timeout"))??;
    Ok(CaptureHandle::new(drop_tx, device_name))
}
