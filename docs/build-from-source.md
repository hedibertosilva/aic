# Build from source (advanced)

Use this path for development or a manual installation. End users can install
release binaries without Rust. Rust 1.88 or later is required (edition 2024 and
let chains); the CI tests with current stable. Install Git, Rust/Cargo and a C
compiler/linker: SQLite is bundled and compiled as part of the build.

```sh
cargo test --locked
cargo build --release --locked
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/aic "$HOME/.local/bin/aic"
```

A glibc executable needs a runtime glibc at least as new as its build environment,
including when aic is mounted inside the agent sandbox. Native static musl builds
are the release format:

```sh
rustup target add x86_64-unknown-linux-musl
# Install musl-tools for your distribution first.
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --locked --target x86_64-unknown-linux-musl
```

On arm64 use `aarch64-unknown-linux-musl` and a native arm64 build machine.
Cross compilation is not provided by installing a Rust target alone.

Copy the actual binary from a distrobox; do not install a distrobox-export wrapper.
A contributor environment may use `distrobox-enter -n apps-env -- cargo test` as
in CLAUDE.md. Node tests: `node --test tests/*.test.cjs`; CLI/browser tests use
`AIC_TEST_BINARY=/absolute/path/to/aic` and Playwright with Chromium.

```sh
scripts/package-release.sh 0.1.0 x86_64-unknown-linux-musl
```

Release CI builds and tests each architecture natively, smoke-tests the binary
in Alpine without Rust and validates checksums before publishing a version tag.
Tag/version mismatch fails publication. Running the workflow manually builds
artifacts only; pushing a `vVERSION` tag publishes the release.
