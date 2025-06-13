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
	"context"
	"fmt"
	"io"
	"log"
	"net/http"
	"sync"
	"time"

	uuid "github.com/satori/go.uuid"
)

// BufferManager manages multiple stream buffers
type BufferManager struct {
	buffers      map[string]*StreamBuffer
	buffersMutex sync.RWMutex
	bufferTime   time.Duration
}

// StreamInfo contains information about a buffered stream
type StreamInfo struct {
	URL        string
	Buffer     *StreamBuffer
	SourceConn *http.Response
	Context    context.Context
	Cancel     context.CancelFunc
}

var globalBufferManager *BufferManager
var bufferManagerOnce sync.Once

// GetBufferManager returns the global buffer manager instance
func GetBufferManager() *BufferManager {
	bufferManagerOnce.Do(func() {
		globalBufferManager = &BufferManager{
			buffers:    make(map[string]*StreamBuffer),
			bufferTime: DefaultBufferDuration,
		}
	})
	return globalBufferManager
}

// SetBufferDuration sets the buffer duration for new streams
func (bm *BufferManager) SetBufferDuration(duration time.Duration) {
	bm.bufferTime = duration
}

// GetOrCreateBuffer gets an existing buffer or creates a new one for a stream URL
func (bm *BufferManager) GetOrCreateBuffer(streamURL string, headers http.Header) (*StreamBuffer, error) {
	bm.buffersMutex.Lock()
	defer bm.buffersMutex.Unlock()

	// Check if buffer already exists
	if buffer, exists := bm.buffers[streamURL]; exists {
		return buffer, nil
	}

	// Create new buffer
	buffer := NewStreamBuffer(bm.bufferTime)
	bm.buffers[streamURL] = buffer

	// Start buffering from the source
	go bm.startBuffering(streamURL, buffer, headers)

	log.Printf("[buffer-manager] Created new buffer for stream: %s", streamURL)
	return buffer, nil
}

// startBuffering starts the buffering process for a stream
func (bm *BufferManager) startBuffering(streamURL string, buffer *StreamBuffer, headers http.Header) {
	defer func() {
		bm.buffersMutex.Lock()
		delete(bm.buffers, streamURL)
		bm.buffersMutex.Unlock()
		buffer.Close()
		log.Printf("[buffer-manager] Stopped buffering for stream: %s", streamURL)
	}()

	for {
		select {
		case <-buffer.ctx.Done():
			return
		default:
			if err := bm.bufferFromSource(streamURL, buffer, headers); err != nil {
				log.Printf("[buffer-manager] Error buffering from source %s: %v", streamURL, err)
				time.Sleep(5 * time.Second) // Wait before retry
				continue
			}
		}
	}
}

// bufferFromSource connects to the source and buffers data
func (bm *BufferManager) bufferFromSource(streamURL string, buffer *StreamBuffer, headers http.Header) error {
	client := &http.Client{
		Timeout: 30 * time.Second,
	}

	req, err := http.NewRequest("GET", streamURL, nil)
	if err != nil {
		return fmt.Errorf("failed to create request: %v", err)
	}

	// Copy headers from original request
	for k, v := range headers {
		for _, vv := range v {
			req.Header.Add(k, vv)
		}
	}

	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("failed to connect to source: %v", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("source returned status %d", resp.StatusCode)
	}

	log.Printf("[buffer-manager] Connected to source %s, status: %d", streamURL, resp.StatusCode)

	// Buffer data in chunks
	buf := make([]byte, DefaultChunkSize)
	for {
		select {
		case <-buffer.ctx.Done():
			return nil
		default:
			n, err := resp.Body.Read(buf)
			if n > 0 {
				if _, writeErr := buffer.Write(buf[:n]); writeErr != nil {
					log.Printf("[buffer-manager] Error writing to buffer: %v", writeErr)
					return writeErr
				}
			}
			if err != nil {
				if err == io.EOF {
					log.Printf("[buffer-manager] Source stream ended for %s", streamURL)
					return nil
				}
				return fmt.Errorf("error reading from source: %v", err)
			}
		}
	}
}

// GetBufferReader creates a new reader for a buffered stream
func (bm *BufferManager) GetBufferReader(streamURL string, headers http.Header) (*BufferReader, error) {
	buffer, err := bm.GetOrCreateBuffer(streamURL, headers)
	if err != nil {
		return nil, err
	}

	readerID := uuid.NewV4().String()
	reader := buffer.NewReader(readerID)

	return reader, nil
}

// RemoveBuffer removes a buffer (called when no more readers)
func (bm *BufferManager) RemoveBuffer(streamURL string) {
	bm.buffersMutex.Lock()
	defer bm.buffersMutex.Unlock()

	if buffer, exists := bm.buffers[streamURL]; exists {
		buffer.Close()
		delete(bm.buffers, streamURL)
		log.Printf("[buffer-manager] Removed buffer for stream: %s", streamURL)
	}
}

// GetStats returns statistics for all buffers
func (bm *BufferManager) GetStats() map[string]interface{} {
	bm.buffersMutex.RLock()
	defer bm.buffersMutex.RUnlock()

	stats := map[string]interface{}{
		"total_buffers": len(bm.buffers),
		"buffer_time":   bm.bufferTime.Seconds(),
		"buffers":       make(map[string]interface{}),
	}

	for url, buffer := range bm.buffers {
		stats["buffers"].(map[string]interface{})[url] = buffer.Stats()
	}

	return stats
}

// BufferedStreamWriter wraps a BufferReader to implement io.Writer interface for gin.Context.Stream
type BufferedStreamWriter struct {
	reader   *BufferReader
	streamURL string
}

// NewBufferedStreamWriter creates a new buffered stream writer
func NewBufferedStreamWriter(streamURL string, headers http.Header) (*BufferedStreamWriter, error) {
	manager := GetBufferManager()
	reader, err := manager.GetBufferReader(streamURL, headers)
	if err != nil {
		return nil, err
	}

	return &BufferedStreamWriter{
		reader:    reader,
		streamURL: streamURL,
	}, nil
}

// Read implements io.Reader interface
func (bsw *BufferedStreamWriter) Read(p []byte) (int, error) {
	return bsw.reader.Read(p)
}

// Close cleans up the reader
func (bsw *BufferedStreamWriter) Close() error {
	if bsw.reader != nil {
		bsw.reader.buffer.RemoveReader(bsw.reader.id)
	}
	return nil
}