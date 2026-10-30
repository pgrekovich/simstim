//! Microphone capture: default input device, resampled to 16 kHz mono.

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

/// Starts capturing from the default input device and sends resampled 16 kHz
/// mono [`Chunk`]s (tagged [`Channel::Me`]) to `tx`.
///
/// The stream lives on a dedicated thread; the returned [`CaptureHandle`]
/// tears it down on drop by signalling that thread over an mpsc channel.
pub fn start_mic(tx: Sender<Chunk>) -> Result<CaptureHandle> {
    let (drop_tx, drop_rx) = channel::<()>();
    let (ready_tx, ready_rx) = channel::<Result<()>>();

    std::thread::spawn(move || {
        let build = || -> Result<cpal::Stream> {
            let host = cpal::default_host();
            let device = host
                .default_input_device()
                .ok_or_else(|| anyhow!("no input device"))?;
            // Exclude the system-audio tap's aggregate device from microphone selection.
            if let Ok(desc) = device.description()
                && desc.name().contains("simstim")
            {
                return Err(anyhow!("default input is our tap"));
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
                |e| eprintln!("mic stream error: {e}"),
                None,
            )?;
            stream.play()?;
            Ok(stream)
        };

        match build() {
            Ok(stream) => {
                let _ = ready_tx.send(Ok(()));
                let _ = drop_rx.recv(); // park until the handle is dropped
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    });

    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| anyhow!("mic init timeout"))??;
    Ok(CaptureHandle::new(drop_tx))
}
