# Roadmap

Owner-ordered priorities as of 2026-08-16. The next stage is decided; the
rest is a ranked backlog, re-ranked after real usage, not a promise. History
and per-stage detail live in the spec's status lines and `docs/plans/`.

## Shipped (MVP-0 through 0.9)

Live local transcription (Parakeet, Me/Them from channel separation), the
notes-app shell over a plain-files store, interim (draft) lines while
speaking, light/dark themes, audio retention (AAC default / FLAC option),
whisper re-transcribe with model selection and deletion, click-to-seek
playback, auto re-transcribe on stop, audio cleanup after re-transcribe,
recording into an existing note with audio and timestamp continuation.

## Next: stage 1.0, call detection (owner's call: the most important thing)

The original Granola magic and the last big piece of daily friction: today
you must remember the app exists before a call starts.

- Detect "an app started using the microphone" via Core Audio process
  objects (no extra permission expected; events over polling if the API
  allows). Match against a configurable whitelist of calling apps (Zoom,
  Slack, Teams, browsers, FaceTime, Discord, ...).
- Notify with the app's name; one click starts recording into a new note.
  **Never auto-record** (product principle 5): detection suggests, the human
  decides. This is also the legal line Granola is being sued over.
- Auto-stop consideration: when the detected app releases the mic, either
  suggest stopping or stop automatically (stopping is the safe direction;
  recording silently is the dangerous one). Decide during design.
- Known limitation to accept up front: a browser using the mic is
  indistinguishable from a call in that browser without an extension.
  Granola notifies anyway with the browser's name; so will we.
- Research: `docs/research/detection.md` (API verification measured before
  the plan was written).

## Ranked backlog (after 1.0)

1. **Menu bar presence + global hotkey.** Detection makes the window
   optional; the tray icon with a recording indicator and a start/stop
   hotkey completes "the app disappears until needed". Pairs naturally with
   1.0 but kept separate so 1.0 stays small.
2. **Enhance (the Granola notes mechanic).** Rough notes + transcript ->
   polished notes via the owner's own API key, into a separate `## Summary`
   section (user text stays sacred). Deliberately postponed until transcript
   quality was worth enhancing; whisper re-transcribe closed that.
3. **Calendar.** EventKit first (free ride on macOS Calendar accounts,
   including Google), full Google OAuth only if EventKit proves not enough.
   Value: pre-titled notes ("Daily stand-up", attendees) instead of
   "Untitled". Explicitly the LAST macOS feature per the original decision.
4. **Windows port** (WASAPI loopback + session notifications), then Linux
   (PipeWire). The capture and detect layers were designed behind traits for
   exactly this; everything else is already portable.
5. **Public release**: signing, notarization, updater, docs, Show HN. The
   niche ("transcribe -> plain md in a folder -> nothing else") was empty as
   of 2026-08; prior-art research says people literally ask for this.

## Smaller known items (do opportunistically, tracked in spec Known issues)

- Measure compression loss on Russian specifically (the 4% WER number was
  measured on other languages).
- Whisper timestamps drift on some segments after re-transcribe (owner
  accepted; revisit only if it starts to hurt seeking).
- Download resume for multi-GB models; sample-rate re-detection when audio
  devices change mid-call; the single-slot suppressed-error stash; FLAC
  padding accumulation on repeated continuations; orphaned
  `.recovering.wav` after a double failure.

## Non-goals (decided, do not resurrect without the owner)

Accounts, cloud sync, telemetry, sharing infrastructure, chat-with-notes,
mobile/watch apps, SQLite anything. The prior-art doc explains why: these
are exactly the features that made every alternative unusable.
