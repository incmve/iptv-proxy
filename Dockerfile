# ---- build ----
FROM rust:1-slim-trixie AS builder
WORKDIR /src

# cargo-auditable embeds the dependency list in the binary, so Trivy can see
# which crates are inside and report CVEs in them.
RUN cargo install cargo-auditable --locked

# Cache dependencies separately from the source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
    && cargo auditable build --release --locked \
    && rm -rf src

COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo auditable build --release --locked

# ---- runtime ----
# distroless/cc: glibc + libgcc, no shell, no package manager.
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=builder /src/target/release/iptv-proxy /usr/local/bin/iptv-proxy

EXPOSE 8080

# The image has no wget/curl; the binary checks /health itself.
HEALTHCHECK --interval=60s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/iptv-proxy", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/iptv-proxy"]
CMD ["serve"]
