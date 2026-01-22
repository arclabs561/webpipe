## Contributing

Thanks for helping improve `webpipe`.

### Development

- **Requirements**: Rust stable toolchain (via `rustup`).
- **Build**:

```bash
cargo build
```

- **Tests**:

```bash
cargo test
```

### Style

- **Format**:

```bash
cargo fmt --all
```

- **Clippy**:

```bash
cargo clippy --all-targets --all-features
```

### Safety + privacy

- Do not commit API keys, `.env` files, or HTTP auth headers.
- Prefer adding tests that run offline (local fixture servers) and keep “live” network tests opt-in.

