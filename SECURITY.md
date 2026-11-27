# Security policy

## Reporting a vulnerability

Please do **not** put the details of a vulnerability in a public issue.

1. Use GitHub's private reporting: the repository's **Security** tab → **Report a vulnerability** (when the button is not there, go to step 2).
2. Otherwise open a public issue titled "Security: please contact me" **without** any detail (no exploit, no file contents, no secrets), and the maintainer
   will answer there with a private way to send it.

What helps: the version (Settings → About shows it), the platform, what you did, what happened, and what you expected. Do not send real keys, tokens,
chats or other people's data.

You can expect an answer within a few days. This is a small project with one maintainer: there is no bug bounty and no guaranteed fix time, but a report
of something that lets a project or a web page run commands, read files or send your login elsewhere will be treated first.

## What is supported

Only the latest release (see [Releases](https://github.com/nathak/karsenia/releases)). Older versions are not patched.

## Scope and known limits

Karsenia runs `claude` (Claude Code) and can therefore run commands through it; it is **not a sandbox**. Read [Security in the README](README.md#security) and
[docs/NOTES.md](docs/NOTES.md#security) first: what the project check does and does not cover, what a trusted project can do, the git-settings watch of
worktree chats (including the one gap that is known and listed there), and the platforms that were never tried (macOS). Reports about those documented limits are
welcome as ideas, but they are not vulnerabilities in themselves.

## Releases

Release programs are built by GitHub Actions on clean runners from the tag of the release ([`.github/workflows/`](.github/workflows/)), with the Rust toolchain
pinned and `--locked`; the Linux test suite runs first. The programs are **not signed by a publisher** (see [docs/signing.md](docs/signing.md)). Check the zip
against its `.sha256` file.
