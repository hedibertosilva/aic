# aic

## Build, tests and install

The host has no linker (`cc`/`ld`); `~/.local/bin/gcc` is a wrapper for the `apps-env` distrobox. Run cargo inside the container:

```
distrobox-enter -n apps-env -- cargo test
distrobox-enter -n apps-env -- cargo build --release
cp target/release/aic ~/.local/bin/aic
```

- Copy the binary straight to the host; don't use `distrobox-export --bin` (see README).
- The sandbox mounts that binary into a Debian trixie image, so prefer the static build: `distrobox-enter -n apps-env -- cargo build --release --target x86_64-unknown-linux-musl` (needs `rustup target add x86_64-unknown-linux-musl` once) and copy `target/x86_64-unknown-linux-musl/release/aic`.
- `cargo check` works on the host, but `cargo test`/`build` only work in the container.
