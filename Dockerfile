FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM alpine:3
COPY --from=build /src/target/release/tache /usr/local/bin/tache
ENV TACHE_BIND=0.0.0.0:8321
EXPOSE 8321
ENTRYPOINT ["tache"]
CMD ["serve"]
