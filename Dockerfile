FROM golang:1.26-alpine AS builder

RUN apk add --no-cache ca-certificates git

WORKDIR /app

COPY . .

# -mod=mod allows go to update go.sum for any missing entries during build
RUN CGO_ENABLED=0 GOOS=linux go build -mod=mod -a -o iptv-proxy .

FROM alpine:3

RUN apk add --no-cache curl wget

COPY --from=builder /app/iptv-proxy /

ENTRYPOINT ["/iptv-proxy"]
