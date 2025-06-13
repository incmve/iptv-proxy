package server

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"testing"
	"time"

	"github.com/incmve/iptv-proxy/pkg/config"
)

func TestStreamBufferingIntegration(t *testing.T) {
	// Create a mock upstream server
	mockServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "video/mp2t")
		// Simulate a continuous stream
		for i := 0; i < 10; i++ {
			w.Write([]byte("chunk" + string(rune('0'+i))))
			if f, ok := w.(http.Flusher); ok {
				f.Flush()
			}
			time.Sleep(100 * time.Millisecond)
		}
	}))
	defer mockServer.Close()

	// Create proxy config with buffering enabled
	proxyConfig := &config.ProxyConfig{
		HostConfig: &config.HostConfiguration{
			Hostname: "localhost",
			Port:     8080,
		},
		User:            "test",
		Password:        "test",
		BufferEnabled:   true,
		BufferDuration:  2, // 2 seconds
		BufferMaxMemory: 10,
	}

	// Create server config
	serverConfig := &Config{
		ProxyConfig: proxyConfig,
	}

	// Test URL
	testURL, _ := url.Parse(mockServer.URL + "/live/stream1")

	t.Run("Should use buffering for live streams", func(t *testing.T) {
		shouldBuffer := serverConfig.shouldUseBuffering(testURL)
		if !shouldBuffer {
			t.Error("Expected buffering to be enabled for live streams")
		}
	})

	t.Run("Should not use buffering when disabled", func(t *testing.T) {
		// Temporarily disable buffering
		originalEnabled := serverConfig.ProxyConfig.BufferEnabled
		serverConfig.ProxyConfig.BufferEnabled = false

		shouldBuffer := serverConfig.shouldUseBuffering(testURL)
		if shouldBuffer {
			t.Error("Expected buffering to be disabled when BufferEnabled is false")
		}

		// Restore original setting
		serverConfig.ProxyConfig.BufferEnabled = originalEnabled
	})

	t.Run("Should not buffer VOD content", func(t *testing.T) {
		vodURL, _ := url.Parse(mockServer.URL + "/movie/12345")
		shouldBuffer := serverConfig.shouldUseBuffering(vodURL)
		if shouldBuffer {
			t.Error("Expected buffering to be disabled for VOD content")
		}
	})

	t.Run("Should not buffer HLS segments", func(t *testing.T) {
		hlsURL, _ := url.Parse(mockServer.URL + "/segment.ts")
		shouldBuffer := serverConfig.shouldUseBuffering(hlsURL)
		if shouldBuffer {
			t.Error("Expected buffering to be disabled for HLS segments")
		}
	})
}

func TestBufferManagerLifecycle(t *testing.T) {
	manager := GetBufferManager()
	manager.SetBufferDuration(1 * time.Second)

	// Test that buffer manager creates and manages buffers
	t.Run("Buffer creation and cleanup", func(t *testing.T) {
		initialStats := manager.GetStats()
		initialCount := initialStats["total_buffers"].(int)

		// Create a mock server for testing
		mockServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Write([]byte("test data"))
		}))
		defer mockServer.Close()

		// Create a buffer reader
		reader, err := manager.GetBufferReader(mockServer.URL, http.Header{})
		if err != nil {
			t.Fatalf("Failed to create buffer reader: %v", err)
		}

		// Check that buffer was created
		newStats := manager.GetStats()
		newCount := newStats["total_buffers"].(int)
		if newCount != initialCount+1 {
			t.Errorf("Expected buffer count to increase by 1, got %d -> %d", initialCount, newCount)
		}

		// Clean up
		reader.buffer.RemoveReader(reader.id)
		
		// Wait a bit for cleanup
		time.Sleep(100 * time.Millisecond)
	})
}