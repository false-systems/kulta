# Build stage
FROM rust:1.83-slim AS builder

# Install dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src

# Build application in release mode
ARG CARGO_PROFILE=release
RUN if [ "$CARGO_PROFILE" = "dev" ]; then \
        cargo build --bin kulta; \
    else \
        cargo build --release --bin kulta; \
    fi

# Runtime stage
FROM debian:bookworm-slim

# Install ca-certificates for HTTPS
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binary from builder
ARG CARGO_PROFILE=release
RUN echo "CARGO_PROFILE is: $CARGO_PROFILE"
COPY --from=builder /app/target/${CARGO_PROFILE}/kulta /app/kulta

# Health check endpoint
EXPOSE 8080
# Metrics endpoint
EXPOSE 9090

# Run the binary
ENTRYPOINT ["/app/kulta"]
