# Contributing

simstim is a small macOS call recorder with a deliberately narrow job.
Reliability, readable files and a calm workflow come first. Document export,
sharing and general AI automation usually belong in the user's existing tools.

## Reporting a bug

[Open an issue](https://github.com/pgrekovich/simstim/issues/new/choose) with:

- simstim version, macOS version and Mac model;
- microphone/headphones and calling app;
- what you did, expected and observed;
- whether Me, Them or both are affected;
- whether the saved audio exists and each channel can be heard.

Use a short synthetic recording if a reproduction needs audio. Do not post
private transcripts, credentials, calendar events or unredacted logs.
For a vulnerability, follow [SECURITY.md](SECURITY.md).

## Proposing a change

Explain the user problem before implementing a new feature. Small fixes can
go directly to a pull request. Larger changes need a discussion of scope.
Keep file format compatibility, separate user/generated text and explicit
recording intact. See [AGENTS.md](AGENTS.md) for architecture and invariants.

Fork, create a branch, make the smallest complete change and run the four
checks in [development.md](docs/development.md). Include a behaviour test
when changing persistence, capture or IPC. Never download models in tests.
Use synthetic data. Describe manual checks and any untested hardware paths.

Use conventional commits and English code, docs and UI copy. Do not add AI
co-author trailers or em dash characters. The maintainer tests a stage on a
real call before merging it. There is no guaranteed response time.

## Building and testing

See [the development guide](docs/development.md). The public beta currently
targets Apple Silicon macOS only.
