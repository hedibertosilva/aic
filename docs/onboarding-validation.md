# Onboarding validation

Validation uses isolated temporary Git checkouts/configuration and fixture CLIs.
Final checks passed: 116 Rust tests and 34 Node/policy/CLI/HTTP/Chromium tests,
without failures or skips; `git diff --check` passed.

No provider generation, OAuth or quota consumption was performed during automated
verification. The existing provider quota captures are regression fixtures.

Rust unit tests cover configuration migration, no workflow fallback, structured
provider events, cancellation, bounded processes, credential-free cache, pretty
Claude auth JSON, Git context and registered worktree validation. Node policy,
CLI/HTTP and Chromium tests cover explicit selection, no agents ready, consent,
cache expiration/invalidation, contextual sandbox results, real failure downgrade,
quota versus logout, timeout/invalid output, interactive setup cancellation,
keyboard recovery, copied commands, drafts, Git acknowledgement, sessions across
worktrees, visibility controls, team shortcuts and skill constraints. Existing
quota, consumption and import tests remain included.

Static musl binaries were built for Linux x86_64 and aarch64. Both executed in
fresh Alpine 3.22.2 root filesystems without Cargo or Rust, using unprivileged
PRoot and QEMU for aarch64. The x86_64 install path was exercised by verifying
checksums, extracting the archive, installing on PATH, initializing a Git checkout,
running setup and doctor with a fixture provider. Checksum instructions use
portable `awk` and `sha256sum -c`, validated with BusyBox as well as GNU tools. This validates installation,
not a real vendor account or a human first-response measurement.

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
