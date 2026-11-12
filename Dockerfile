# syntax=docker/dockerfile:1

# Alpine's native target is musl, so the binaries are linked statically and
# run in an image that contains nothing else.
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --config profile.release.strip=true \
        -p slicer-server -p slicer-cli \
    && mkdir /out \
    && cp target/release/spaghetti-slicer-server target/release/spaghetti-slicer /out/

FROM scratch
COPY --from=build /out/ /
# Any user without a name will do; there is nothing in the image to own.
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/spaghetti-slicer-server"]
