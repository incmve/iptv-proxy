FROM golang:1.24-alpine AS builder

RUN apk add --no-cache ca-certificates git

WORKDIR /app

# Copy dependency manifests first so Docker can cache the download layer
# independently of source changes.
COPY go.mod go.sum ./
# Download all modules declared in go.mod and extend go.sum with any missing
# checksums (go.sum is currently out of sync: gin v1.9→v1.12, google/uuid
# absent, cors/cobra/viper stale). This populates the module cache so the
# subsequent build does not need outbound network access.
RUN go mod download

COPY . .

# TODO: run `go mod vendor` to sync the vendor directory with go.mod, then
# switch this back to -mod=vendor for reproducible offline builds.
RUN CGO_ENABLED=0 GOOS=linux go build -mod=mod -a -o iptv-proxy .

FROM alpine:3

RUN apk add --no-cache curl wget

COPY --from=builder /app/iptv-proxy /

ENTRYPOINT ["/iptv-proxy"]
