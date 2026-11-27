# Code signing (SignPath Foundation): where the repository stands

Status: **no signing is active.** The Windows `.exe` is unsigned, no signing key or certificate exists anywhere (repository, CI, secrets), and
nothing in the README or the releases says otherwise. This page is the preparation for **applying** to the free open-source program of
[SignPath Foundation](https://signpath.org/terms) (conditions read on 2026-11-27; they can change, read the page again before applying). It
changes nothing by itself.

## 1. The conditions, and what the repository has

| SignPath condition | Karsenia today | Verdict |
|---|---|---|
| OSI-approved license **without commercial dual-licensing for all components** | own code: MIT. The desktop app links **Slint**, used here under the **Slint Royalty-free Desktop License** (not OSI-approved). Slint itself is offered as GPLv3 **or** Royalty-free **or** a paid commercial license | **blocker, see 2** |
| No proprietary, non-open-source component | the Royalty-free license is a proprietary license text (free, with attribution) | **blocker, see 2** |
| Actively maintained; already released in the form to be signed; functionality documented on the download page | releases 0.1.0–0.1.4, a detailed README, one maintainer | likely ok (one maintainer; ask whether that is enough) |
| Sign your own binaries only; the team owns the repository and the build scripts | the Windows build is ours (`packaging/windows-build/`, `.github/workflows/`); `zig` is only the compiler | ok |
| Build in a verifiable way, on GitHub-hosted runners, from the tagged source | `release.yml`: clean `ubuntu-24.04` runners, tag must equal the `Cargo.toml` version, Rust pinned, `--locked` | ok (SignPath can enforce `require_github_hosted`) |
| Every release approved manually before signing | a SignPath signing policy with a manual approver | to set up in SignPath |
| Signed binaries carry enforced metadata (product name, version) | the `.exe` has an icon resource but **no version-info resource** (`build.rs` writes the icon only) | **to do** (see 3) |
| No hidden data transfer; anything sent to systems the user did not name is in a privacy policy and can be switched off | audited in the code (v0.1.1 and v0.1.4): the app has no HTTP/TLS client, no analytics, crash reporting or update check; its sockets are local; everything that goes out is done by `claude` (or a gateway/web task the user configured) and by plug-in installs through `claude`. It is worth stating in the policy that the app starts `claude` by itself at start-up (named chats, no message) and when Settings or the usage panel is opened | ok, to state in the policy |
| No system changes without warning; **uninstall instructions** if there are install instructions | portable zip, nothing installed | add one line on removal to the README and the read-me in the zip (see 3) |
| MFA for every team member on GitHub **and** SignPath | not verifiable from the repository | the maintainer must confirm |
| Roles: Authors, Reviewers, Approvers | one maintainer would be all three | state it in the policy; ask whether a solo project is accepted |
| A **"Code signing policy"** on the project home page and the download/release pages (with the roles, a privacy statement and the line "Free code signing provided by SignPath.io, certificate by SignPath Foundation") | none yet | write it **when** approved (draft in 4); do not publish it before signing is real |

## 2. The license question (the one that decides)

- The repository's own code is MIT. That alone fits. The **distributed program**, however, is that code **plus Slint**, and Slint is not MIT:
  Slint's files are under GPLv3, the Royalty-free License or a commercial license, **at the user's choice** (Slint's own `LICENSE.md` and FAQ).
- Karsenia uses the **Royalty-free Desktop License** (free; condition: show the Slint attribution, which Settings → About does with the
  `AboutSlint` widget; no use in embedded systems; the application must not expose Slint's APIs). It is a free license but **not OSI-approved**, and it
  is a license of a company that also sells a commercial one. Against SignPath's "OSI-approved license … without commercial dual-licensing for all
  components" and "no proprietary component", this is the most likely reason for a refusal. **I cannot decide it from the documents; SignPath can.**
- The way out Slint itself describes: use Slint under **GPLv3**. Then, per Slint's FAQ, "your own files can stay MIT, but the work as a whole must be
  licensed under the GPL": the **distributed binary becomes GPL-3.0** (the MIT files stay MIT and are GPL-compatible). That makes the binary
  OSI-approved, but it is a decision for the owner, not a technical step: it changes what recipients may do with the program (copyleft), what the README
  and `LICENSE` say, and it cannot be undone for versions already released under the Royalty-free choice. Whether SignPath also counts Slint's
  own commercial option as "commercial dual-licensing of a component" is a question to ask them.
- Third-party notices: the program contains many Rust crates (MIT, Apache-2.0, …) whose license texts should travel with the binary. The zips carry
  only `FONTS-LICENSE.txt`. A generated notices file (for example `cargo about`) would close that gap; not needed for SignPath as such, but a reviewer
  may ask.

- **If the binary becomes GPL-3.0 (checked 2026-11-27):** the dependency tree (`cargo metadata --features desktop`, all targets, 620 crates) is MIT, Apache-2.0 (compatible
  with GPLv3), Unicode-3.0, Zlib, BSD, Unlicense, BSL-1.0, 0BSD, CC0 and ISC, plus the Slint crates, whose **GPL-3.0-only** option would be the one taken; `r-efi` is
  `MIT OR Apache-2.0 OR LGPL-2.1-or-later` (the permissive choice is taken). No license in the tree conflicts with GPL-3.0. Slint's GPL option is "-only", so the
  combined work would be GPL-3.0-**only**. SignPath's project list shows GPL-3.0 projects (for example LunaBox) and Qt-based applications (Qt is also dual-licensed),
  which suggests that using a dual-licensed library under its open-source option is accepted in practice; the terms do not say so, so ask SignPath.

**Decision for the owner (before applying):** (A) keep the Royalty-free choice and ask SignPath whether it is acceptable, expecting a refusal; or
(B) release future versions under GPLv3 for the combined binary (and keep MIT for the own files), then apply; or (C) do not use SignPath (a paid
certificate, or no signing). Nothing here has been changed.

## 3. Work that is useful whatever the answer (not done yet)

1. **Version-info resource in the `.exe`** (product name, version, company/publisher line): extend `build.rs` where the icon resource is made.
   It also shows in Explorer's Properties → Details today's unsigned file.
2. **Removal instructions**: done in the README and in the read-me templates (`packaging/READ-ME-FIRST*.txt`); the zips carry them from the next release on.
3. **Third-party notices** file (see 2).
4. `SECURITY.md` (how to report a vulnerability) and `CONTRIBUTING.md` (how changes are reviewed): they describe the Authors/Reviewers/Approvers roles in practice.

## 4. How a signing step would fit (not added: it would be untested code, and it needs an account)

The release already has the right shape: `windows` job → artifact → `publish` job. Signing goes **between** them, with the SignPath action pinned to a
commit and the secrets only in that job:

```
windows:  build the exe → make the unsigned zip → upload-artifact (unsigned)
sign:     needs windows · signpath/github-action-submit-signing-request@<commit> (api-token from a secret, organization-id, project-slug,
          signing-policy-slug, github-artifact-id = the unsigned artifact, wait-for-completion) → the signed zip
publish:  needs sign · compute the .sha256 of the SIGNED zip · create the release
```

Details to settle with SignPath's documentation at that time: the artifact configuration (a `<zip-file>` holding `karsenia-desktop.exe` as `<pe-file>`),
the `.sha256` made **after** signing (the packaging script does it from the zip it makes, so signing must come before that or the script must run again on the
signed exe), the signing policy's manual approval, and `disallow_reruns`. macOS is not covered by SignPath (it needs an Apple developer account and
notarization).

## 5. Draft of the "Code signing policy" (publish only after approval)

> **Code signing policy.** Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).
> The Windows program `karsenia-desktop.exe` in the releases is built by GitHub Actions from the tagged source of this repository
> (`.github/workflows/release.yml`) and signed only from that build. Roles: Authors and Reviewers: the maintainer(s) listed in the repository; Approver:
> the maintainer, who approves every signing request by hand. Privacy: the program itself sends nothing anywhere on its own; it runs `claude` (Claude
> Code), which talks to Anthropic as its own documentation says, and, only if you configure them, a model gateway or a web task at the address you give.
> (Wording verified against the code in the privacy audit: the app has no HTTP or TLS client of its own; its only sockets are local; the traffic is that of `claude`, of a gateway or web task you configured, and of a plug-in install that `claude` performs.)

## 6. What the owner must do

Confirm: MFA on GitHub (and later on SignPath); the license decision in 2; who the Authors/Reviewers/Approver are; whether to publish the policy page; and
that the SignPath application (https://signpath.org/apply) is to be filed by the owner, since it needs the owner's identity and accounts.
