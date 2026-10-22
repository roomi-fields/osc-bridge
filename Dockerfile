# osc-bridge MCP server in a container — used by MCP directories (Glama) to
# run and score the server. The catalogue tools (list_devices, list_routes,
# get_device_docs) work as is; hardware tools need MIDI devices, which a
# container normally does not see. `docker run -p 8080:8080 <image> --http
# 0.0.0.0:8080` serves the catalogue tools over HTTP instead of stdio.
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends libasound2-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends libasound2 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/osc-bridge /opt/osc-bridge/bin/osc-bridge
COPY devices /opt/osc-bridge/devices
EXPOSE 8080
ENTRYPOINT ["/opt/osc-bridge/bin/osc-bridge", "mcp"]
