package server

import (
	"testing"
	"time"
)

func TestStreamBuffer(t *testing.T) {
	t.Run("Buffer creation", func(t *testing.T) {
		buffer := NewStreamBuffer(5 * time.Second)
		if buffer == nil {
			t.Fatal("Failed to create stream buffer")
		}
		defer buffer.Close()

		stats := buffer.Stats()
		if stats["buffer_time"] != 5.0 {
			t.Errorf("Expected buffer time 5.0, got %v", stats["buffer_time"])
		}
	})

	t.Run("Write and read", func(t *testing.T) {
		buffer := NewStreamBuffer(1 * time.Second)
		defer buffer.Close()

		// Write some data
		testData := []byte("Hello, World!")
		n, err := buffer.Write(testData)
		if err != nil {
			t.Fatalf("Failed to write to buffer: %v", err)
		}
		if n != len(testData) {
			t.Errorf("Expected to write %d bytes, wrote %d", len(testData), n)
		}

		// Create a reader
		reader := buffer.NewReader("test-reader")
		if reader == nil {
			t.Fatal("Failed to create buffer reader")
		}

		// Wait for buffer delay
		time.Sleep(1100 * time.Millisecond)

		// Read data
		readBuf := make([]byte, len(testData))
		n, err = reader.Read(readBuf)
		if err != nil {
			t.Fatalf("Failed to read from buffer: %v", err)
		}
		if n != len(testData) {
			t.Errorf("Expected to read %d bytes, read %d", len(testData), n)
		}
		if string(readBuf[:n]) != string(testData) {
			t.Errorf("Expected to read %q, got %q", string(testData), string(readBuf[:n]))
		}
	})

	t.Run("Buffer manager", func(t *testing.T) {
		manager := GetBufferManager()
		if manager == nil {
			t.Fatal("Failed to get buffer manager")
		}

		stats := manager.GetStats()
		if stats["total_buffers"] == nil {
			t.Error("Buffer manager stats should include total_buffers")
		}
	})
}

func TestBufferConfiguration(t *testing.T) {
	t.Run("Buffer duration configuration", func(t *testing.T) {
		manager := GetBufferManager()
		
		// Test setting buffer duration
		testDuration := 3 * time.Second
		manager.SetBufferDuration(testDuration)

		stats := manager.GetStats()
		if stats["buffer_time"] != 3.0 {
			t.Errorf("Expected buffer time 3.0, got %v", stats["buffer_time"])
		}
	})
}