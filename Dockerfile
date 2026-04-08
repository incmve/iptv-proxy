FROM golang:1.26-alpine AS builder

RUN apk add --no-cache ca-certificates git

WORKDIR /app

COPY . .

# TODO: run `go mod vendor` to sync the vendor directory with go.mod, then
# switch this back to -mod=vendor for reproducible offline builds.
# Currently -mod=mod is required because vendor/modules.txt is out of sync
# (gin v1.9.0→v1.12.0, cors/cobra/viper stale, google/uuid missing entirely).
RUN CGO_ENABLED=0 GOOS=linux go build -mod=mod -a -o iptv-proxy .

FROM alpine:3

RUN apk add --no-cache curl wget

COPY --from=builder /app/iptv-proxy /

ENTRYPOINT ["/iptv-proxy"]
