# aic

## Build, tests and install

The host has no linker (`cc`/`ld`); `~/.local/bin/gcc` is a wrapper for the `apps-env` distrobox. Run cargo inside the container:

```
distrobox-enter -n apps-env -- cargo test
distrobox-enter -n apps-env -- cargo build --release
cp target/release/aic ~/.local/bin/aic
```

- Copy the binary straight to the host; don't use `distrobox-export --bin` (see README).
- `cargo check` works on the host, but `cargo test`/`build` only work in the container.
