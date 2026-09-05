# Records the mdmanager.ai workflow demo from a clean installation.
FROM rust:1.91.1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY docs ./docs
COPY tests ./tests
RUN cargo build --release --locked

FROM ghcr.io/charmbracelet/vhs:v0.11.0
RUN apt-get update && apt-get install -y --no-install-recommends tmux git fonts-dejavu-core && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/mdmanager /usr/local/bin/mdmanager
COPY homepage/demo /demo
WORKDIR /demo
ENTRYPOINT ["vhs"]
