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
	"io/ioutil"
	"log"
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

func (c *Config) reverseProxy(ctx *gin.Context) {
	rpURL, err := url.Parse(c.track.URI)
	if err != nil {
		ctx.AbortWithError(http.StatusInternalServerError, err) // nolint: errcheck
		return
	}

	c.stream(ctx, rpURL)
}

func (c *Config) m3u8ReverseProxy(ctx *gin.Context) {
	id := ctx.Param("id")

	rpURL, err := url.Parse(strings.ReplaceAll(c.track.URI, path.Base(c.track.URI), id))
	if err != nil {
		ctx.AbortWithError(http.StatusInternalServerError, err) // nolint: errcheck
		return
	}

	c.stream(ctx, rpURL)
}

func (c *Config) stream(ctx *gin.Context, oriURL *url.URL) {
	// Check if buffering is enabled for this stream
	if c.shouldUseBuffering(oriURL) {
		c.streamWithBuffer(ctx, oriURL)
		return
	}

	// Fall back to direct streaming
	c.streamDirect(ctx, oriURL)
}

func (c *Config) streamDirect(ctx *gin.Context, oriURL *url.URL) {
	client := &http.Client{}

	req, err := http.NewRequest("GET", oriURL.String(), nil)
	if err != nil {
		ctx.AbortWithError(http.StatusInternalServerError, err) // nolint: errcheck
		return
	}

	mergeHttpHeader(req.Header, ctx.Request.Header)

	resp, err := client.Do(req)
	if err != nil {
		ctx.AbortWithError(http.StatusInternalServerError, err) // nolint: errcheck
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
	// Create buffered stream writer
	bufferedWriter, err := NewBufferedStreamWriter(oriURL.String(), ctx.Request.Header)
	if err != nil {
		log.Printf("[stream] Failed to create buffered writer for %s: %v", oriURL.String(), err)
		// Fall back to direct streaming
		c.streamDirect(ctx, oriURL)
		return
	}
	defer bufferedWriter.Close()

	// Pre-buffer data before starting playback
	preloadDuration := time.Duration(c.ProxyConfig.BufferPreload) * time.Second
	if preloadDuration > 0 {
		log.Printf("[stream] Pre-buffering %v seconds for %s", preloadDuration, oriURL.String())
		
		// Wait for buffer to accumulate data
		startTime := time.Now()
		for time.Since(startTime) < preloadDuration {
			// Check if buffer has data or if we should timeout
			select {
			case <-ctx.Done():
				log.Printf("[stream] Client disconnected during pre-buffering")
				return
			default:
				time.Sleep(100 * time.Millisecond) // Check every 100ms
			}
		}
		log.Printf("[stream] Pre-buffering complete, starting playback")
	}

	// Set appropriate headers
	ctx.Header("Content-Type", "video/mp2t") // Default to MPEG-TS for IPTV
	ctx.Header("Cache-Control", "no-cache")
	ctx.Header("Connection", "keep-alive")
	
	log.Printf("[stream] Starting buffered stream for %s", oriURL.String())

	// Stream buffered data to client
	ctx.Stream(func(w io.Writer) bool {
		buf := make([]byte, DefaultChunkSize)
		n, err := bufferedWriter.Read(buf)
		if n > 0 {
			w.Write(buf[:n]) // nolint: errcheck
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
	// Check if buffering is globally enabled
	if !c.ProxyConfig.BufferEnabled {
		return false
	}
	
	// Enable buffering for live streams but not for VOD/series content
	urlPath := oriURL.Path
	
	// Skip buffering for HLS segments and manifest files
	if strings.HasSuffix(urlPath, ".m3u8") || strings.HasSuffix(urlPath, ".ts") {
		return false
	}
	
	// Skip buffering for movie/series content (usually large files)
	if strings.Contains(urlPath, "/movie/") || strings.Contains(urlPath, "/series/") {
		return false
	}
	
	// Enable buffering for live TV streams
	if strings.Contains(urlPath, "/live/") || strings.Contains(urlPath, "/get.php") {
		return true
	}
	
	// Default: enable buffering for live content
	return true
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
		ctx.AbortWithError(http.StatusBadRequest, err) // nolint: errcheck
		return
	}
	if c.ProxyConfig.User.String() != authReq.Username || c.ProxyConfig.Password.String() != authReq.Password {
		ctx.AbortWithStatus(http.StatusUnauthorized)
	}
}

func (c *Config) appAuthenticate(ctx *gin.Context) {
	contents, err := ioutil.ReadAll(ctx.Request.Body)
	if err != nil {
		ctx.AbortWithError(http.StatusInternalServerError, err) // nolint: errcheck
		return
	}

	q, err := url.ParseQuery(string(contents))
	if err != nil {
		ctx.AbortWithError(http.StatusInternalServerError, err) // nolint: errcheck
		return
	}
	if len(q["username"]) == 0 || len(q["password"]) == 0 {
		ctx.AbortWithError(http.StatusBadRequest, fmt.Errorf("bad body url query parameters")) // nolint: errcheck
		return
	}
	log.Printf("[iptv-proxy] %v | %s |App Auth\n", time.Now().Format("2006/01/02 - 15:04:05"), ctx.ClientIP())
	if c.ProxyConfig.User.String() != q["username"][0] || c.ProxyConfig.Password.String() != q["password"][0] {
		ctx.AbortWithStatus(http.StatusUnauthorized)
	}

	ctx.Request.Body = ioutil.NopCloser(bytes.NewReader(contents))
}

func (c *Config) bufferStats(ctx *gin.Context) {
	manager := GetBufferManager()
	stats := manager.GetStats()
	ctx.JSON(http.StatusOK, stats)
}
