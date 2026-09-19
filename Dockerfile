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

# Stage 3: Runtime - minimal image
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false acteon
WORKDIR /app
COPY --from=builder /app/target/release/acteon-server /usr/local/bin/acteon-server
COPY --from=ui-builder /ui/dist /app/ui/dist
USER acteon
EXPOSE 8080
ENTRYPOINT ["acteon-server"]
