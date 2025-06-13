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
	"io"
	"log"
	"sync"
	"time"
)

const (
	// DefaultBufferDuration is the default buffer duration in seconds
	DefaultBufferDuration = 5 * time.Second
	// DefaultChunkSize is the size of each buffer chunk in bytes
	DefaultChunkSize = 32 * 1024 // 32KB chunks
	// MaxBufferMemory is the maximum memory per stream buffer
	MaxBufferMemory = 10 * 1024 * 1024 // 10MB per stream
)

// BufferChunk represents a single chunk of buffered data
type BufferChunk struct {
	Data      []byte
	Timestamp time.Time
	Size      int
}

// StreamBuffer implements a ring buffer for stream data
type StreamBuffer struct {
	chunks        []*BufferChunk
	writeIndex    int
	readIndex     int
	size          int
	capacity      int
	bufferTime    time.Duration
	chunkSize     int
	totalBytes    int64
	mutex         sync.RWMutex
	readers       map[string]*BufferReader
	readersMutex  sync.RWMutex
	lastWrite     time.Time
	closed        bool
	ctx           context.Context
	cancel        context.CancelFunc
}

// BufferReader represents a client reading from the buffer
type BufferReader struct {
	id         string
	readIndex  int
	lastRead   time.Time
	buffer     *StreamBuffer
}

// NewStreamBuffer creates a new stream buffer
func NewStreamBuffer(bufferDuration time.Duration) *StreamBuffer {
	if bufferDuration <= 0 {
		bufferDuration = DefaultBufferDuration
	}

	// Calculate capacity based on buffer duration and estimated bitrate
	// Assume average bitrate of 2Mbps for capacity calculation
	estimatedBytesPerSecond := 250 * 1024 // 2Mbps / 8 = 250KB/s
	estimatedTotalBytes := int(bufferDuration.Seconds()) * estimatedBytesPerSecond
	capacity := estimatedTotalBytes / DefaultChunkSize
	if capacity < 10 {
		capacity = 10 // Minimum capacity
	}
	if capacity > MaxBufferMemory/DefaultChunkSize {
		capacity = MaxBufferMemory / DefaultChunkSize // Maximum capacity
	}

	ctx, cancel := context.WithCancel(context.Background())

	buffer := &StreamBuffer{
		chunks:     make([]*BufferChunk, capacity),
		capacity:   capacity,
		bufferTime: bufferDuration,
		chunkSize:  DefaultChunkSize,
		readers:    make(map[string]*BufferReader),
		ctx:        ctx,
		cancel:     cancel,
	}

	// Start cleanup goroutine
	go buffer.cleanup()

	return buffer
}

// Write writes data to the buffer
func (sb *StreamBuffer) Write(data []byte) (int, error) {
	sb.mutex.Lock()
	defer sb.mutex.Unlock()

	if sb.closed {
		return 0, io.ErrClosedPipe
	}

	totalWritten := 0
	now := time.Now()

	for len(data) > 0 {
		chunkData := data
		if len(chunkData) > sb.chunkSize {
			chunkData = data[:sb.chunkSize]
		}

		chunk := &BufferChunk{
			Data:      make([]byte, len(chunkData)),
			Timestamp: now,
			Size:      len(chunkData),
		}
		copy(chunk.Data, chunkData)

		// Replace old chunk if buffer is full
		if sb.chunks[sb.writeIndex] != nil {
			sb.totalBytes -= int64(sb.chunks[sb.writeIndex].Size)
		}

		sb.chunks[sb.writeIndex] = chunk
		sb.totalBytes += int64(chunk.Size)

		sb.writeIndex = (sb.writeIndex + 1) % sb.capacity
		if sb.size < sb.capacity {
			sb.size++
		} else {
			// Buffer is full, move read index
			sb.readIndex = (sb.readIndex + 1) % sb.capacity
		}

		totalWritten += len(chunkData)
		data = data[len(chunkData):]
	}

	sb.lastWrite = now
	return totalWritten, nil
}

// NewReader creates a new reader for this buffer
func (sb *StreamBuffer) NewReader(id string) *BufferReader {
	sb.readersMutex.Lock()
	defer sb.readersMutex.Unlock()

	sb.mutex.RLock()
	defer sb.mutex.RUnlock()

	reader := &BufferReader{
		id:       id,
		buffer:   sb,
		lastRead: time.Now(),
	}

	// Start reading from a position that's bufferTime behind current write
	now := time.Now()
	targetTime := now.Add(-sb.bufferTime)

	// Find the appropriate starting position
	reader.readIndex = sb.findReadPosition(targetTime)

	sb.readers[id] = reader
	log.Printf("[buffer] New reader %s created, starting at index %d", id, reader.readIndex)

	return reader
}

// findReadPosition finds the best read position for a given target time
func (sb *StreamBuffer) findReadPosition(targetTime time.Time) int {
	if sb.size == 0 {
		return sb.readIndex
	}

	// Find the chunk closest to but not newer than target time
	bestIndex := sb.readIndex
	bestDiff := time.Duration(1 << 62) // Max duration

	for i := 0; i < sb.size; i++ {
		index := (sb.readIndex + i) % sb.capacity
		chunk := sb.chunks[index]
		if chunk == nil {
			continue
		}

		if chunk.Timestamp.Before(targetTime) || chunk.Timestamp.Equal(targetTime) {
			diff := targetTime.Sub(chunk.Timestamp)
			if diff < bestDiff {
				bestDiff = diff
				bestIndex = index
			}
		}
	}

	return bestIndex
}

// Read reads data from the buffer for a specific reader
func (br *BufferReader) Read(p []byte) (int, error) {
	br.buffer.mutex.RLock()
	defer br.buffer.mutex.RUnlock()

	if br.buffer.closed {
		return 0, io.EOF
	}

	// Check if we need to wait for buffer delay
	now := time.Now()
	if br.readIndex < br.buffer.size {
		chunk := br.buffer.chunks[(br.buffer.readIndex+br.readIndex)%br.buffer.capacity]
		if chunk != nil {
			timeSinceWrite := now.Sub(chunk.Timestamp)
			if timeSinceWrite < br.buffer.bufferTime {
				// Not enough time has passed, return no data for now
				return 0, nil
			}
		}
	}

	if br.readIndex >= br.buffer.size {
		// No more data available
		return 0, nil
	}

	chunkIndex := (br.buffer.readIndex + br.readIndex) % br.buffer.capacity
	chunk := br.buffer.chunks[chunkIndex]
	if chunk == nil {
		return 0, nil
	}

	bytesToCopy := len(chunk.Data)
	if bytesToCopy > len(p) {
		bytesToCopy = len(p)
	}

	copy(p[:bytesToCopy], chunk.Data[:bytesToCopy])
	br.readIndex++
	br.lastRead = now

	return bytesToCopy, nil
}

// RemoveReader removes a reader from the buffer
func (sb *StreamBuffer) RemoveReader(id string) {
	sb.readersMutex.Lock()
	defer sb.readersMutex.Unlock()

	delete(sb.readers, id)
	log.Printf("[buffer] Reader %s removed", id)

	// If no readers left, we can consider closing the buffer
	if len(sb.readers) == 0 {
		go func() {
			time.Sleep(30 * time.Second) // Grace period
			sb.readersMutex.RLock()
			stillEmpty := len(sb.readers) == 0
			sb.readersMutex.RUnlock()

			if stillEmpty {
				sb.Close()
			}
		}()
	}
}

// Close closes the buffer and stops all operations
func (sb *StreamBuffer) Close() error {
	sb.mutex.Lock()
	defer sb.mutex.Unlock()

	if sb.closed {
		return nil
	}

	sb.closed = true
	sb.cancel()

	log.Printf("[buffer] Buffer closed, processed %d bytes total", sb.totalBytes)
	return nil
}

// cleanup runs periodic cleanup tasks
func (sb *StreamBuffer) cleanup() {
	ticker := time.NewTicker(10 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-sb.ctx.Done():
			return
		case <-ticker.C:
			sb.cleanupStaleReaders()
		}
	}
}

// cleanupStaleReaders removes readers that haven't read data recently
func (sb *StreamBuffer) cleanupStaleReaders() {
	sb.readersMutex.Lock()
	defer sb.readersMutex.Unlock()

	now := time.Now()
	staleThreshold := 2 * time.Minute

	for id, reader := range sb.readers {
		if now.Sub(reader.lastRead) > staleThreshold {
			delete(sb.readers, id)
			log.Printf("[buffer] Removed stale reader %s", id)
		}
	}
}

// Stats returns buffer statistics
func (sb *StreamBuffer) Stats() map[string]interface{} {
	sb.mutex.RLock()
	defer sb.mutex.RUnlock()

	sb.readersMutex.RLock()
	defer sb.readersMutex.RUnlock()

	return map[string]interface{}{
		"capacity":     sb.capacity,
		"size":         sb.size,
		"total_bytes":  sb.totalBytes,
		"readers":      len(sb.readers),
		"buffer_time":  sb.bufferTime.Seconds(),
		"last_write":   sb.lastWrite,
	}
}