FROM ubuntu:24.04

RUN apt-get update && apt-get install -y \
    git curl build-essential ca-certificates sudo tmux jq \
    && rm -rf /var/lib/apt/lists/*

# Node 22 (for claude CLI)
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs

# Claude Code CLI
RUN npm install -g @anthropic-ai/claude-code

# gh CLI
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | tee /etc/apt/sources.list.d/github-cli.list > /dev/null \
    && apt-get update && apt-get install -y gh \
    && rm -rf /var/lib/apt/lists/*

# State-reporting helper for worker containers
COPY lib/container/sipag-state.sh /usr/local/bin/sipag-state
RUN chmod +x /usr/local/bin/sipag-state

# Non-root user (claude refuses --dangerously-skip-permissions as root)
RUN useradd -m -s /bin/bash sipag
USER sipag

WORKDIR /work
