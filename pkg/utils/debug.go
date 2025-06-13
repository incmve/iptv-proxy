package utils

import (
	"log"

	"github.com/incmve/iptv-proxy/pkg/config"
)

// Logs a message if debug logging is enabled
func DebugLog(format string, v ...interface{}) {
	if config.DebugLoggingEnabled {
		log.Printf("[DEBUG] "+format, v...)
	}
}
