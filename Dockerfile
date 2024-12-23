# First stage: Build the Go application
FROM golang:1.17-alpine AS builder

# Install required tools
RUN apk add --no-cache ca-certificates gcc musl-dev

# Set the working directory
WORKDIR /go/src/github.com/incmve/iptv-proxy

# Force Go to use the vendor directory
ENV GOFLAGS="-mod=vendor"

# Copy the entire project into the container
COPY . .

# Build the Go application
RUN CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo -o iptv-proxy .

# Second stage: Create a minimal runtime image
FROM alpine:3

# Copy the built binary from the builder stage
COPY --from=builder /go/src/github.com/incmve/iptv-proxy/iptv-proxy /

# Set the entrypoint
ENTRYPOINT ["/iptv-proxy"]
