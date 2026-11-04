# charet-agent-run: what one web agent run works in. The coding agents (Claude Agent SDK, Codex
# SDK), the charet CLI and skill, and the runner that ties them to a run. No network but the
# charet-agent relay and proxy.

FROM node:24-trixie-slim AS cli
RUN npm install --global pnpm@12.5.1
WORKDIR /src
COPY . .
RUN pnpm install --frozen-lockfile --prod --filter "charet-cli..."

FROM node:24-trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git ripgrep \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/runner
RUN npm install --omit=dev --no-audit --no-fund \
      @anthropic-ai/claude-agent-sdk@0.3.318 @openai/codex-sdk@0.166.0 \
    && npm cache clean --force
COPY --from=cli /src/node_modules /opt/charet/node_modules
COPY --from=cli /src/apps/charet-cli /opt/charet/apps/charet-cli
COPY --from=cli /src/packages/i18n /opt/charet/packages/i18n
COPY --from=cli /src/packages/api-client /opt/charet/packages/api-client
COPY apps/charet-agent/src /opt/runner/src
COPY skills/charet /home/node/.claude/skills/charet
COPY skills/charet /home/node/.codex/skills/charet
RUN printf '#!/bin/sh\nexec node /opt/charet/apps/charet-cli/src/main.ts "$@"\n' > /usr/local/bin/charet \
    && chmod 755 /usr/local/bin/charet \
    && mkdir -p /work \
    && chown -R node:node /work /home/node
USER node
ENV CHARET_RUN_SPEC=/input/spec.json \
    CHARET_RUN_WORK=/work/project
CMD ["node", "/opt/runner/src/run/main.ts"]
