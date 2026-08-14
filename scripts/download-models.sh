#!/usr/bin/env bash
set -euo pipefail
DIR="$HOME/Library/Application Support/simstim/models"
mkdir -p "$DIR"

if [ ! -f "$DIR/silero_vad.onnx" ]; then
  curl -L -o "$DIR/silero_vad.onnx" \
    https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx
fi

if [ ! -d "$DIR/parakeet-tdt-0.6b-v3-int8" ]; then
  curl -L -o /tmp/parakeet.tar.bz2 \
    https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2
  tar -xjf /tmp/parakeet.tar.bz2 -C "$DIR"
  mv "$DIR/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8" "$DIR/parakeet-tdt-0.6b-v3-int8"
  rm /tmp/parakeet.tar.bz2
fi
echo "models ready in $DIR"
