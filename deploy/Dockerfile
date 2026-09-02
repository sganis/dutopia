# deploy/Dockerfile
# Runtime-only image: it packages the binaries already built by
# deploy/build-linux.sh. No compilation happens here.
#
# Build it on the SAME Linux host that produced dist/, so the binary's
# architecture and glibc requirements match:
#
#   ./deploy/build-linux.sh
#   podman build -f deploy/Dockerfile -t registry.corp.local/dutopia/duapi:4.0.26 .
#
# The base must provide a glibc at least as new as the build host's.
# build-linux.sh prints the exact floor. Rule of thumb:
#   built on RHEL/Rocky 9, Alma 9   -> ubi9/ubi-minimal   (glibc 2.34)
#   built on RHEL/Rocky 8           -> ubi8/ubi-minimal   (glibc 2.28)
#   built on Ubuntu 22.04           -> ubuntu:22.04       (glibc 2.35)
#   built on Ubuntu 24.04           -> ubuntu:24.04       (glibc 2.39)
FROM registry.access.redhat.com/ubi9/ubi-minimal:latest

LABEL org.opencontainers.image.title="dutopia-duapi" \
      org.opencontainers.image.source="https://github.com/sganis/dutopia"

WORKDIR /app
COPY dist/duapi dist/dudb /app/
COPY dist/public /app/public

# OpenShift runs the container as a random UID in group 0, so everything the
# process reads must be group-readable and group-executable. Do not drop this.
RUN chgrp -R 0 /app && chmod -R g=u /app

ENV STATIC_DIR=/app/public \
    PORT=8080 \
    DB_PATH=/data/data.db
EXPOSE 8080
USER 1001
ENTRYPOINT ["/app/duapi"]
