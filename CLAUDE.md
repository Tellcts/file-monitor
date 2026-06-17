# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
cargo build          # Build the project
cargo run            # Build and run
cargo test           # Run all tests
cargo test <name>    # Run a single test by name
cargo fmt            # Format code
cargo clippy         # Lint
```

## Architecture

- **Crate**: `file_monitor` (binary)
- **Rust edition**: 2024
- **Entry point**: `src/main.rs` — currently just a "Hello, world!" placeholder
- **Dependencies**: none yet
