# Technology choices, with the evidence

Every entry here was measured or verified, mostly on the owner's machine
(Apple M1 Max, macOS 26.5). Do not redo this work without a concrete reason;
if you do re-measure, update this file with the new numbers.

## Audio capture (macOS)

**Core Audio process taps** (`CATapDescription` + `AudioHardwareCreateProcessTap`
+ a private aggregate device + IOProc), via the pure-Rust `cidre` bindings,
plus `cpal` for the microphone. macOS 14.2+ only.

Rejected: ScreenCaptureKit (drags in screen-recording permission and the
recording indicator, and captures a polluted full mix), BlackHole-style
virtual drivers (install friction, obsolete since macOS 13).

Traps, all of which cost real debugging time:

- `tap_auto_start` must be **false**. With true, the synchronous
  `AudioDeviceStart` blocks until some app produces audio, so starting a
  recording hangs indefinitely.
- The tap's aggregate device can appear in the input device list; the mic
  path filters any device whose name contains ours, or it records itself.
- Permission lives under "System Audio Recording Only" and is granted per
  **code-signing identity**. Unsigned dev builds inherit the terminal's
  identity, terminals have no `NSAudioCaptureUsageDescription`, so macOS
  shows no prompt at all and the tap silently returns zeros.

## Live ASR: Parakeet TDT 0.6b v3 via sherpa-onnx

Chosen for the owner's language mix (English, Russian, mixed). Covers 25
European languages with punctuation and auto language ID at 600M parameters,
and runs comfortably faster than real time on Apple Silicon.

- The official `sherpa-onnx` Rust crate (not the deprecated `sherpa-rs`).
- `model_type: "nemo_transducer"` is required, silently wrong without it.
- Silero VAD gates speech into chunks; true streaming models covering Russian
  did not exist as of mid-2026, so VAD-chunked pseudo-streaming is what
  everyone ships. Interim decoding of the in-progress buffer is what makes it
  feel live.
- Known limitation: **Parakeet's vocabulary contains no "ё" at all**, so it
  emits `<unk>` where that letter was spoken. The engine restores it as "е"
  (standard Russian normalization). This is architectural, not tunable.
- `max_speech_duration` does not reliably split continuous speech, so the
  session worker force-finalizes at 10 s as a backstop.

## Offline pass: whisper large-v3 family via whisper-rs

- `whisper-rs 0.16` with the `metal` feature. Builds in ~35 s with only Xcode
  Command Line Tools; ggml-metal JIT-compiles its shaders at runtime, so no
  full Xcode is needed.
- Measured: large-v3-turbo q8 transcribes at about **12x realtime** on the M1
  Max, so a 30 minute call takes about 2.5 minutes. Segment timestamps come
  back correctly (centiseconds).
- sherpa-onnx's own whisper support was rejected: its published models carry
  **no timestamps at all**, which makes a transcript impossible.
- Offered lineup is deliberately two models: turbo (faster) and large-v3
  (best on non-English). Quantization variants were retired from the UI
  because the difference is 0.1 to 0.3 WER, i.e. noise, and choosing between
  them is a coin flip dressed up as a decision.

## Audio storage format

Measured on 10 minutes of speech, transcribed with whisper and compared
against the lossless reference (WER = word error rate versus that reference):

| Format | MB/hour | WER vs lossless | Plays natively on macOS |
|---|---|---|---|
| FLAC (lossless) | 137 | 0 % | yes |
| Opus 24k | 11 | 45 % | yes (macOS 26) |
| Opus 32k | 15 | 5.3 % | yes (macOS 26) |
| **AAC 32k (16 kHz)** | **17** | **3.8 %** | **yes** |
| AAC 64k | 33 | 4.3 % | yes |
| MP3 32k | 14 | 4.2 % | yes |
| MP3 64k | 29 | 3.9 % | yes |

Conclusions that drove the decision:

- **Raising the bitrate does not buy accuracy back** (AAC 64k is no better
  than 32k). The real choice is lossless or not, and the price of "not" is
  about 4 % of words.
- Opus at 24 kbps is a quality cliff (whisper starts repeating and
  hallucinating), not a cheaper option.
- MP3 is disqualified: LAME omits the gapless header at 32 kbps, so decoded
  audio drifts ~69 ms at that bitrate but not at 64 kbps. A silent,
  bitrate-dependent timing shift is unacceptable in an app that pins audio
  position to transcript timestamps. LGPL is a second strike.
- AAC won over Opus on native-playback certainty and zero licensing, at
  effectively the same size and accuracy.

Default is **AAC 32 kbps forced to 16 kHz**; FLAC remains an option.
`afconvert` silently drops to a 12 kHz internal rate at 32 kbps unless the
rate is forced, so the encoder always passes `aac@16000` and verifies the
result.

AAC priming delay was measured as **0 samples** (impulse round trip), so
click-to-seek stays accurate.

Not measured: how much of that 4 % applies to Russian specifically. The
benchmark had no Russian sample. Worth one measurement on a real call before
anyone trusts the number for Russian.

## Framework

Tauri 2 + Rust + React. Every serious local-transcription project converged
here (Anarlog, Meetily, Vibe, screenpipe): ~15 MB installers and 20 to 100 MB
idle RAM against Electron's 120+, with the Rust side owning capture and ASR
in the same process. LynxJS was considered and rejected: its desktop support
is four months old, its own roadmap still lists production readiness as
future work, and its full-app desktop story runs on Electron anyway.
