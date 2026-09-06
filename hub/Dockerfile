# holler-hub: kleines Rust-Binary ohne Abhängigkeiten, statisch gebaut.
# Bauen aus dem Repo-Stamm:  docker build -f hub/Dockerfile -t holler-hub .
FROM rust:1-slim AS build
WORKDIR /src
COPY hub/ hub/
WORKDIR /src/hub
RUN cargo build --release --locked 2>/dev/null || cargo build --release

FROM debian:bookworm-slim
COPY --from=build /src/hub/target/release/holler-hub /usr/local/bin/holler-hub
EXPOSE 4712/udp
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/holler-hub"]
CMD ["--port", "4712"]
