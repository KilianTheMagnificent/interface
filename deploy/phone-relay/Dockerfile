FROM rust:1.88-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p interface-remote --bin interface-relay

FROM debian:bookworm-slim
COPY --from=build /build/target/release/interface-relay /usr/local/bin/interface-relay
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/interface-relay"]
