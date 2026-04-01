FROM golang:1.26-alpine AS builder

RUN apk add --no-cache ca-certificates git

WORKDIR /app

# Copy module definition first — this layer is cached until go.mod changes
COPY go.mod ./
# Regenerates go.sum and downloads all dependencies
RUN go mod tidy

COPY . .
RUN CGO_ENABLED=0 GOOS=linux go build -a -o iptv-proxy .

FROM alpine:3

RUN apk add --no-cache curl wget

COPY --from=builder /app/iptv-proxy /

ENTRYPOINT ["/iptv-proxy"]
