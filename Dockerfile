# Use the Golang image to build your project
FROM golang:1.17-alpine AS builder

# Install necessary packages
RUN apk add --no-cache ca-certificates

# Set the working directory
WORKDIR /go/src/github.com/incmve/iptv-proxy

# Copy the source code into the container
COPY . .

# Build the project
RUN GO111MODULE=off CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo -o iptv-proxy .

# Use the Alpine image for the final stage
FROM alpine:3

# Install necessary packages including curl
RUN apk add --no-cache curl

# Copy the built binary from the builder stage
COPY --from=builder /go/src/github.com/incmve/iptv-proxy/iptv-proxy /

# Set the entry point for the container
ENTRYPOINT ["/iptv-proxy"]
