# Stage 1: UI Builder
FROM node:22-bookworm-slim AS ui-builder
WORKDIR /ui
COPY ui/package*.json ./
RUN npm ci
COPY ui/ .
RUN npm run build

# Stage 2: Chef - dependency caching
FROM rust:1.88-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

# Stage 3: Planner - generate recipe
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 4: Builder - build dependencies and application
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release -p acteon-server

# Stage 5: Runtime - minimal image
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false acteon
WORKDIR /app
COPY --from=builder /app/target/release/acteon-server /usr/local/bin/acteon-server
COPY --from=ui-builder /ui/dist /app/ui/dist
USER acteon
EXPOSE 8080
ENTRYPOINT ["acteon-server"]
