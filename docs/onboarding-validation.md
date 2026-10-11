# Onboarding validation

Validation uses isolated temporary Git checkouts/configuration and fixture CLIs.
Second-review checks passed: 116 Rust tests and 41 Node/policy/CLI/HTTP/Chromium tests,
without failures or skips; `git diff --check` passed.

The latest new-chat refinement passed 24 Node policy/import tests and all 8
onboarding Chromium tests, without skips. New chats wait for Add participant or
Import conversation card actions, including first load and the keyboard shortcut.
Coverage includes missing/unavailable defaults, no ready agents, import without
participant setup, and multiline instructions through save, edit, request payload
and reload. The second-review configuration and Environment regression tests
also passed.

For this refinement Cargo was unavailable. Browser requests for HTML/CSS/JS were
routed to the current `src/chat/web` files, with the existing debug binary serving
the fixture API. Chromium and its dependencies were provisioned in temporary
directories. No Rust tests or packaged-binary rebuild are claimed for this round;
the earlier Rust evidence below belongs to the previous validation. Rebuild and
restart `aic` to embed the updated web assets in the application binary.

No provider generation, OAuth or quota consumption was performed during automated
verification. The existing provider quota captures are regression fixtures.

Rust unit tests cover configuration migration, no workflow fallback, structured
provider events, cancellation, bounded processes, credential-free cache, pretty
Claude auth JSON, Git context and registered worktree validation. Node policy,
CLI/HTTP and Chromium tests cover explicit selection, no agents ready, consent,
cache expiration/invalidation, contextual sandbox results, real failure downgrade,
quota versus logout, timeout/invalid output, interactive setup cancellation,
keyboard recovery, copied commands, drafts, Git acknowledgement, sessions across
worktrees, protected default visibility, team configuration through participants
and skill constraints. Browser tests verify that Environment never changes empty
or populated conversations, participants, roles, native sessions or drafts. They
measure dialog padding, viewport bounds and internal scrolling on short mobile
screens, verify keyboard recovery and keep checkout selection under the current
conversation's Git context → Advanced. The second review adds staged visibility
with Cancel/Esc, rejected and successful Save, persisted reload behavior, direct
alternative-agent selection and relevant-only Environment status. Optional unused
agents stay in Environment; quota stays solely in Usage limits. Desktop and mobile
screenshots were inspected for the conversation disclosure and selection warning;
the disclosure's mobile bounds were verified within the viewport.
Integration tests count auth/container invocations: preflight and sends perform
no auth probes or diagnostic container launches, validate only recipients, and
reject expired tests, changed binaries/images and unregistered paths. Existing
quota, consumption and import tests remain included.

During the initial distribution validation, static musl binaries were built for
Linux x86_64 and aarch64. Both executed in
fresh Alpine 3.22.2 root filesystems without Cargo or Rust, using unprivileged
PRoot and QEMU for aarch64. The x86_64 install path was exercised by verifying
checksums, extracting the archive, installing on PATH, initializing a Git checkout,
running setup and doctor with a fixture provider. Checksum instructions use
portable `awk` and `sha256sum -c`, validated with BusyBox as well as GNU tools. This validates installation,
not a real vendor account or a human first-response measurement.
The interface refactoring was verified with a fresh debug build; the earlier
distribution artifacts were not repackaged or published during this revision.

Release CI builds each architecture natively; ARM64 local verification used Zig
for cross compilation and QEMU for execution. The artifact is static; compatibility
of vendor executables and containers is still verified separately by readiness.
Checksums are generated in `dist/SHA256SUMS`. Publication awaits remote access:
the read-only `git ls-remote --tags origin` attempt returned `Host key verification
failed`. No tags, commits or releases were pushed.

Security checks include same-origin rejection, registered-path enforcement,
native-session provenance, independent quota/authentication, restricted cache
permissions, process/container cleanup and absence of raw vendor stderr/secrets
in diagnostic JSON, stored results and displayed errors. Clippy completes with
pre-existing repository style warnings; a strict `-D warnings` check is not clean.

## Human usability protocol (pending)

Recruit someone familiar with Git and terminal, with a supported Linux system,
Git repository, network and at least one provider account. Start timing when
obtaining the binary; finish when the first valid chat response arrives. Record
third-party CLI installation and OAuth time separately from product-controlled
time and total elapsed time. Capture errors, requests for help, agent selection,
understanding of installation/authentication/execution/quota and Git context.

Do not edit TOML or coach during the task. Exercise an unavailable preferred
agent and a branch change as recovery scenarios. Report whether the initial
10-minute goal is met and list improvements if it is not. Automation timings are
not evidence of this usability target. T301, T302 and T303 remain unchecked until
a person completes the test and measurements are recorded.
