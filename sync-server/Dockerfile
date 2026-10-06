# Lorekeeper sync server. Build from the repository root (it needs sync-protocol too):
#   docker build -f sync-server/Dockerfile -t lorekeeper-sync .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY sync-protocol sync-protocol
COPY sync-server sync-server
RUN cargo build --release --locked --manifest-path sync-server/Cargo.toml \
 && install -D sync-server/target/release/lorekeeper-sync /out/lorekeeper-sync \
 && mkdir /data

# glibc and libgcc only, no shell; same Debian release as the builder.
FROM gcr.io/distroless/cc-debian12
LABEL org.opencontainers.image.title="Lorekeeper sync server" \
      org.opencontainers.image.description="Stores and relays a Lorekeeper party's end-to-end encrypted campaign notes" \
      org.opencontainers.image.source="https://github.com/yonatankarp/lorekeeper" \
      org.opencontainers.image.url="https://yonatankarp.com/lorekeeper/"
COPY --from=build /out/lorekeeper-sync /usr/local/bin/lorekeeper-sync
# /data is the only place the server writes (database plus its WAL and SHM files), so the root
# filesystem can be read-only. A fresh named volume takes this owner.
COPY --from=build --chown=10001:10001 /data /data
USER 10001:10001
ENV LOREKEEPER_SYNC_DB=/data/sync.db \
    LOREKEEPER_SYNC_ADDR=0.0.0.0:8080
EXPOSE 8080
VOLUME /data
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["/usr/local/bin/lorekeeper-sync", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/lorekeeper-sync"]
