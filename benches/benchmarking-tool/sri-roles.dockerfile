FROM rust:latest as builder
WORKDIR usr/src/stratum/
COPY ../.. .
WORKDIR roles/
RUN cargo build --release

FROM rust:latest
COPY --from=builder /usr/src/stratum/roles/ /usr/src/stratum/roles
WORKDIR /usr/src/stratum/roles
