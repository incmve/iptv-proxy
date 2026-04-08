FROM golang:1.24-alpine AS builder

RUN apk add --no-cache ca-certificates git

WORKDIR /app

COPY . .

RUN CGO_ENABLED=0 GOOS=linux go build -mod=vendor -a -o iptv-proxy .

FROM alpine:3

RUN apk add --no-cache curl wget

COPY --from=builder /app/iptv-proxy /

ENTRYPOINT ["/iptv-proxy"]
