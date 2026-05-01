package main

// RFC 6238 TOTP generator — no external dependencies, pure stdlib.
// The secret is the raw Base32 key stored in the authenticator app
// (NOT a 6-digit code), e.g. "JBSWY3DPEHPK3PXP".

import (
	"crypto/hmac"
	"crypto/sha1"
	"encoding/base32"
	"encoding/binary"
	"fmt"
	"strings"
	"time"
)

// generateTOTP returns the current 6-digit TOTP code for the given
// Base32 secret using a 30-second window and HMAC-SHA1.
func generateTOTP(secret string) (string, error) {
	// Normalise: upper-case, strip spaces / dashes / equals signs.
	secret = strings.ToUpper(strings.ReplaceAll(
		strings.ReplaceAll(
			strings.ReplaceAll(secret, " ", ""),
			"-", ""),
		"=", ""))

	// Decode without padding first (most apps omit it).
	key, err := base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(secret)
	if err != nil {
		// Pad to a multiple of 8 and retry.
		for len(secret)%8 != 0 {
			secret += "="
		}
		key, err = base32.StdEncoding.DecodeString(secret)
		if err != nil {
			return "", fmt.Errorf("decode TOTP secret: %w", err)
		}
	}

	// Counter = floor(unix / 30).
	counter := uint64(time.Now().Unix()) / 30
	buf := make([]byte, 8)
	binary.BigEndian.PutUint64(buf, counter)

	mac := hmac.New(sha1.New, key)
	mac.Write(buf)
	h := mac.Sum(nil)

	// Dynamic truncation → 31-bit integer.
	offset := h[len(h)-1] & 0x0f
	code := (int(h[offset])&0x7f)<<24 |
		(int(h[offset+1])&0xff)<<16 |
		(int(h[offset+2])&0xff)<<8 |
		(int(h[offset+3]) & 0xff)

	return fmt.Sprintf("%06d", code%1_000_000), nil
}
