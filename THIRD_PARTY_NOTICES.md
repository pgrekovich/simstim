# Third-party acknowledgements

simstim's MIT license applies to its own code. Dependencies, fonts and model
weights retain their respective licenses and notices.

- [Tauri](https://github.com/tauri-apps/tauri), [React](https://github.com/facebook/react)
  and their dependencies provide the desktop shell and interface.
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx),
  [Silero VAD](https://github.com/snakers4/silero-vad),
  [Parakeet](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3),
  [whisper.cpp](https://github.com/ggml-org/whisper.cpp) and
  [whisper-rs](https://github.com/tazz4843/whisper-rs) make local transcription possible.
- [cidre](https://github.com/yury/cidre), [cpal](https://github.com/RustAudio/cpal)
  and [tauri-nspanel](https://github.com/ahkohd/tauri-nspanel) provide native integration.
- [Anarlog](https://github.com/fastrepl/anarlog) and
  [AudioCap](https://github.com/insidegui/AudioCap) informed the macOS capture work.
- Source Serif 4 and Spline Sans Mono ship with their
  [font licenses](apps/desktop/src/assets/fonts/README.md).

The exact dependency versions are recorded in `Cargo.lock` and
`apps/desktop/pnpm-lock.yaml`. Model weights are downloaded separately from
the URLs in the model registry; consult their model cards and licenses.
Claude, ChatGPT, Codex and other product names belong to their owners.
simstim is an independent project.
