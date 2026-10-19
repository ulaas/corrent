# Corrent Server in a container.
#   docker build -f packaging/server/docker/Dockerfile -t corrent-server .
#   docker run -d --name corrent -p 8443:8443 -p 6881:6881 -p 6881:6881/udp \
#     -v corrent-data:/data -v /srv/media:/media corrent-server
# First run: create the administrator
#   docker exec -it corrent correntd --data-dir /data --init-admin admin
# Containers aren't on the LAN by default: open https://<host>:8443 and pair apps by address
# (mDNS discovery needs --network host).

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p correntd -p corrent-mpv --bin correntd --bin corrent-mpvw

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libmpv2 ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --home /data --uid 1000 corrent \
 && mkdir -p /data /media && chown corrent:corrent /data
COPY --from=build /src/target/release/correntd /src/target/release/corrent-mpvw /usr/local/bin/
USER corrent
VOLUME ["/data"]
EXPOSE 8443 6881 6881/udp
ENV CORRENT_LOG=info
ENTRYPOINT ["/usr/bin/tini", "--", "correntd", "--data-dir", "/data"]
