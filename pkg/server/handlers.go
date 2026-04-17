/*
 * Iptv-Proxy is a project to proxyfie an m3u file and to proxyfie an Xtream iptv service (client API).
 * Copyright (C) 2020  Pierre-Emmanuel Jacquier
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

package server

import (
	"bytes"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"path"
	"strings"
	"time"

	"github.com/gin-gonic/gin"
)

func (c *Config) getM3U(ctx *gin.Context) {
	ctx.Header("Content-Disposition", fmt.Sprintf(`attachment; filename=%q`, c.M3UFileName))
	ctx.Header("Content-Type", "application/octet-stream")

	ctx.File(c.proxyfiedM3UPath)
}

// sharedTransport is a single connection pool shared across all upstream requests.
// This prevents FD exhaustion from per-request transports accumulating idle connections.
// No client-level Timeout is set: http.Client.Timeout is a total request lifetime deadline
// that would kill live streams after 30 s. Connection establishment is bounded by DialContext.
var sharedTransport = &http.Transport{
	DialContext: (&net.Dialer{
		Timeout:   10 * time.Second,
		KeepAlive: 30 * time.Second,
	}).DialContext,
	MaxIdleConns:        100,
	MaxIdleConnsPerHost: 10,
	IdleConnTimeout:     90 * time.Second,
}

// streamClient is the shared client for direct stream proxying.
var streamClient = &http.Client{
	Transport: sharedTransport,
}

// hlsClient is the shared client for HLS streams; it does not follow redirects
// so that redirect location headers can be inspected and re-routed.
var hlsClient = &http.Client{
	Transport: sharedTransport,
	CheckRedirect: func(req *http.Request, via []*http.Request) error {
		return http.ErrUseLastResponse
	},
}

func (c *Config) reverseProxy(ctx *gin.Context) {
	rpURL, err := url.Parse(c.track.URI)
	if err != nil {
		ctx.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "internal error", "detail": err.Error()})
		return
	}

	c.stream(ctx, rpURL)
}

func (c *Config) m3u8ReverseProxy(ctx *gin.Context) {
	id := ctx.Param("id")

	rpURL, err := url.Parse(strings.ReplaceAll(c.track.URI, path.Base(c.track.URI), id))
	if err != nil {
		ctx.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "internal error", "detail": err.Error()})
		return
	}

	c.stream(ctx, rpURL)
}

func (c *Config) stream(ctx *gin.Context, oriURL *url.URL) {
	if c.shouldUseBuffering(oriURL) {
		c.streamWithBuffer(ctx, oriURL)
		return
	}
	c.streamDirect(ctx, oriURL)
}

func (c *Config) streamDirect(ctx *gin.Context, oriURL *url.URL) {
	req, err := http.NewRequestWithContext(ctx.Request.Context(), "GET", oriURL.String(), nil)
	if err != nil {
		ctx.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "internal error", "detail": err.Error()})
		return
	}

	mergeHttpHeader(req.Header, ctx.Request.Header)

	resp, err := streamClient.Do(req)
	if err != nil {
		ctx.AbortWithStatusJSON(http.StatusServiceUnavailable, gin.H{"error": "upstream unavailable", "detail": err.Error()})
		return
	}
	defer resp.Body.Close()

	mergeHttpHeader(ctx.Writer.Header(), resp.Header)
	ctx.Status(resp.StatusCode)
	ctx.Stream(func(w io.Writer) bool {
		io.Copy(w, resp.Body) // nolint: errcheck
		return false
	})
}

func (c *Config) streamWithBuffer(ctx *gin.Context, oriURL *url.URL) {
	bufferedWriter, err := NewBufferedStreamWriter(oriURL.String(), ctx.Request.Header)
	if err != nil {
		log.Printf("[stream] Failed to create buffered writer for %s: %v, falling back to direct", oriURL.String(), err)
		c.streamDirect(ctx, oriURL)
		return
	}
	defer bufferedWriter.Close()

	// Pre-buffer before starting playback
	if preload := time.Duration(c.ProxyConfig.BufferPreload) * time.Second; preload > 0 {
		deadline := time.Now().Add(preload)
		for time.Now().Before(deadline) {
			select {
			case <-ctx.Done():
				return
			default:
				time.Sleep(100 * time.Millisecond)
			}
		}
	}

	ctx.Header("Content-Type", "video/mp2t")
	ctx.Header("Cache-Control", "no-cache")
	ctx.Header("Connection", "keep-alive")

	buf := make([]byte, DefaultChunkSize)
	ctx.Stream(func(w io.Writer) bool {
		n, err := bufferedWriter.Read(buf)
		if n > 0 {
			w.Write(buf[:n]) // nolint: errcheck
		} else if err == nil {
			// No data yet (buffer delay); yield to avoid pegging CPU.
			time.Sleep(10 * time.Millisecond)
		}
		if err != nil {
			if err != io.EOF {
				log.Printf("[stream] Buffer read error: %v", err)
			}
			return false
		}
		return true
	})
}

func (c *Config) shouldUseBuffering(oriURL *url.URL) bool {
	if !c.ProxyConfig.BufferEnabled {
		return false
	}

	p := oriURL.Path

	// Skip HLS segments and manifests — these need precise timing
	if strings.HasSuffix(p, ".m3u8") || strings.HasSuffix(p, ".ts") {
		return false
	}

	// Skip VOD content
	if strings.Contains(p, "/movie/") || strings.Contains(p, "/series/") {
		return false
	}

	return true
}

func (c *Config) bufferStats(ctx *gin.Context) {
	ctx.JSON(http.StatusOK, GetBufferManager().GetStats())
}

func (c *Config) xtreamStream(ctx *gin.Context, oriURL *url.URL) {
	id := ctx.Param("id")
	if strings.HasSuffix(id, ".m3u8") {
		c.hlsXtreamStream(ctx, oriURL)
		return
	}

	c.stream(ctx, oriURL)
}

type values []string

func (vs values) contains(s string) bool {
	for _, v := range vs {
		if v == s {
			return true
		}
	}

	return false
}

func mergeHttpHeader(dst, src http.Header) {
	for k, vv := range src {
		for _, v := range vv {
			if values(dst.Values(k)).contains(v) {
				continue
			}
			dst.Add(k, v)
		}
	}
}

// authRequest handle auth credentials
type authRequest struct {
	Username string `form:"username" binding:"required"`
	Password string `form:"password" binding:"required"`
}

func (c *Config) authenticate(ctx *gin.Context) {
	var authReq authRequest
	if err := ctx.Bind(&authReq); err != nil {
		ctx.AbortWithStatusJSON(http.StatusBadRequest, gin.H{"error": "bad request", "detail": err.Error()})
		return
	}
	if c.ProxyConfig.User.String() != authReq.Username || c.ProxyConfig.Password.String() != authReq.Password {
		ctx.AbortWithStatus(http.StatusUnauthorized)
		return
	}
}

func (c *Config) appAuthenticate(ctx *gin.Context) {
	contents, err := io.ReadAll(ctx.Request.Body)
	if err != nil {
		ctx.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "internal error", "detail": err.Error()})
		return
	}

	q, err := url.ParseQuery(string(contents))
	if err != nil {
		ctx.AbortWithStatusJSON(http.StatusInternalServerError, gin.H{"error": "internal error", "detail": err.Error()})
		return
	}
	if len(q["username"]) == 0 || len(q["password"]) == 0 {
		ctx.AbortWithStatusJSON(http.StatusBadRequest, gin.H{"error": "bad request", "detail": "missing username or password"})
		return
	}
	log.Printf("[iptv-proxy] %v | %s |App Auth\n", time.Now().Format("2006/01/02 - 15:04:05"), ctx.ClientIP())
	if c.ProxyConfig.User.String() != q["username"][0] || c.ProxyConfig.Password.String() != q["password"][0] {
		ctx.AbortWithStatus(http.StatusUnauthorized)
		return
	}

	ctx.Request.Body = io.NopCloser(bytes.NewReader(contents))
}
