# syntax=docker/dockerfile:1

# Base images are pinned by digest; Dependabot keeps them up to date.
FROM rust:1.98-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS builder

# protoc compiles the metadata schema.
# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends protobuf-compiler \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

# The registry and build caches persist between builds on the same builder;
# the binary is copied out because cache mounts are not part of the image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p yeet-yoink \
 && cp target/release/yeet-yoink /usr/local/bin/yeet-yoink

# glibc and CA certificates, no shell, and an unprivileged user (65532).
FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f

COPY --from=builder /usr/local/bin/yeet-yoink /usr/local/bin/yeet-yoink

# The HTTP API, and gossip between instances.
EXPOSE 8080/tcp 7946/udp

ENTRYPOINT ["/usr/local/bin/yeet-yoink"]
CMD ["--http", "0.0.0.0:8080"]
