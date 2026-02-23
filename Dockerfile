# Stage 1: Build sipag-worker binary
FROM rust:1.85-bookworm AS builder

WORKDIR /build

# Copy workspace root
COPY Cargo.toml Cargo.lock ./

# Copy the crates we need to build
COPY sipag-core/ sipag-core/
COPY sipag-worker/ sipag-worker/

# Stub out sipag/ and tui/ so Cargo can resolve the workspace
RUN mkdir -p sipag/src tui/src \
    && echo 'fn main() {}' > sipag/src/main.rs \
    && echo 'fn main() {}' > tui/src/main.rs
COPY sipag/Cargo.toml sipag/Cargo.toml
COPY tui/Cargo.toml tui/Cargo.toml

# Copy prompts (needed by include_str! in sipag-worker)
COPY lib/prompts/ lib/prompts/

# Build only the sipag-worker binary
RUN cargo build --release --package sipag-worker \
    && strip target/release/sipag-worker

# Stage 2: Runtime image
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y \
    git curl build-essential ca-certificates tmux gnupg locales \
    && locale-gen en_US.UTF-8 \
    && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8 \
    LANGUAGE=en_US:en \
    LC_ALL=en_US.UTF-8

# Node 22 (for claude CLI) — proper GPG-signed apt source, no curl|bash
RUN curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
    | gpg --dearmor -o /usr/share/keyrings/nodesource.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" \
    | tee /etc/apt/sources.list.d/nodesource.list > /dev/null \
    && apt-get update && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

# Claude Code CLI
RUN npm install -g @anthropic-ai/claude-code

# gh CLI
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | tee /etc/apt/sources.list.d/github-cli.list > /dev/null \
    && apt-get update && apt-get install -y gh \
    && rm -rf /var/lib/apt/lists/*

# sipag-worker binary (replaces worker.sh + sipag-state.sh)
COPY --from=builder /build/target/release/sipag-worker /usr/local/bin/sipag-worker

# Non-root user (claude refuses --dangerously-skip-permissions as root)
RUN useradd -m -s /bin/bash sipag \
    && mkdir -p /work && chown sipag:sipag /work
USER sipag

WORKDIR /work
