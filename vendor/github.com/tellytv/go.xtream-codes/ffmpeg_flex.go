package xtreamcodes

import (
	"bytes"
	"encoding/json"
)

// UnmarshalJSON accepts the normal object form of FFMPEGStreamInfo, but also
// tolerates providers that send an empty array ([]), any array, null or an
// empty string for "audio"/"video". Those cases yield an empty struct instead
// of failing the whole get_series_info / get_vod_info response.
func (fsi *FFMPEGStreamInfo) UnmarshalJSON(data []byte) error {
	trimmed := bytes.TrimSpace(data)
	if len(trimmed) == 0 ||
		trimmed[0] == '[' ||
		bytes.Equal(trimmed, []byte("null")) ||
		bytes.Equal(trimmed, []byte(`""`)) {
		*fsi = FFMPEGStreamInfo{}
		return nil
	}

	type alias FFMPEGStreamInfo // alias has no methods, so no recursion
	var a alias
	if err := json.Unmarshal(trimmed, &a); err != nil {
		// Malformed stream metadata is not worth failing the whole response.
		*fsi = FFMPEGStreamInfo{}
		return nil
	}
	*fsi = FFMPEGStreamInfo(a)
	return nil
}
