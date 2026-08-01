export CARGO_BUILD_JOBS := "2"

check:
    cargo check --workspace

build:
    cargo build --workspace

test:
    cargo test --workspace

lint:
    cargo clippy --workspace --all-targets -- -D warnings
