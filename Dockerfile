# Stage 1: UI Builder
FROM node:22-bookworm-slim AS ui-builder
WORKDIR /ui
COPY ui/package*.json ./
RUN npm ci
COPY ui/ .
RUN npm run build

# Stage 2: Build the server with the repository's lockfile.
FROM rust:1.88-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake pkg-config libssl-dev libcurl4-openssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --locked --release -p acteon-server

# Stage 3: Runtime - glibc, OpenSSL, C++ runtime, and CA roots without a shell.
# Keep the distro explicit and update this digest with container scan validation.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:54df941ed0d06a1bd95ef5e0ce391fd8d9f94b64782dc9a60062727849ee3f97 AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/acteon-server /usr/local/bin/acteon-server
COPY --from=ui-builder /ui/dist /app/ui/dist
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/acteon-server"]
