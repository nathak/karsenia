# Contributing

Thank you for looking. This is a small project with one maintainer; an issue before a big change saves everyone's time.

## License of contributions

Karsenia is licensed under **GPL-3.0-only** (see [LICENSE](LICENSE)). By sending a change (a pull request, a patch) you agree that it is licensed under the same
terms, and that you wrote it or have the right to send it. If it contains code from somewhere else, say where from and under which license; code under a license
that is not compatible with GPL-3.0-only cannot be accepted. (Until version 0.1.4 the source code was published under the MIT license; those releases stay as they
were published.)

## How a change gets in

- Changes to `main` are made by the maintainer or arrive as pull requests; a pull request from anyone else is **reviewed by the maintainer** before it is merged.
- A release is made only from a tag by the release workflow, and the maintainer decides every release. Nothing is signed (see [docs/signing.md](docs/signing.md)).

## Before you send a pull request

```sh
cargo test --features desktop --locked         # no model calls: a fake `claude` replays scenarios
node --test tests/ui.test.mjs                  # the web page's pure helpers
cargo clippy --features desktop --locked --all-targets   # the warnings that are there now are known; do not add new ones
```

- Keep a change small and say **why** in the description. Code comments and commit messages are in English; the user-facing texts exist in Korean and English
  (`src/view/i18n.rs`, `ui/app.js`).
- Add a test with the behaviour you change (the tests use real `git` in temporary folders and a fake `claude`; see `tests/`).
- Do not put secrets, real chats, e-mail addresses or paths of your machine in code, tests, screenshots or issues.
- Windows and macOS are not covered by automated tests: say what you tried by hand.
- Design notes and the reasons behind the rules are in [docs/NOTES.md](docs/NOTES.md); the board's design is in [docs/design/board.md](docs/design/board.md).

## Reporting a problem

Use the issue tracker, with the version, the platform and `<data folder>/logs/desktop.log` (after removing anything private). For a security problem see
[SECURITY.md](SECURITY.md).
