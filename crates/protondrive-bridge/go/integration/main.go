// Integration test for ProtonDrive-Linux.
//
// Connects to the real Proton Drive API using credentials supplied via
// environment variables:
//
//	PROTON_USER      – Proton account email
//	PROTON_PASSWORD  – account password
//	PROTON_KEY       – TOTP secret key (base-32, NOT a one-time code)
//
// The test:
//  1. Generates a TOTP code from PROTON_KEY.
//  2. Attempts login; if Proton challenges with a HumanVerification captcha
//     (error 9001) it solves the drag-puzzle using headless Chrome.
//  3. Lists the root Drive folder.
//  4. Asserts that at least one item is returned (proves the API returns
//     real data, not an empty stub).
//  5. Prints a summary of the first 10 items found.
//
// Exit code: 0 on success, 1 on any failure.
package main

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"time"

	bridge "github.com/henrybear327/Proton-API-Bridge"
	"github.com/henrybear327/Proton-API-Bridge/common"
	proton "github.com/henrybear327/go-proton-api"
	"github.com/go-resty/resty/v2"
)

const (
	appVersion = "external-drive-protondrive-linux@0.1.23-stable"
	userAgent  = "ProtonDrive-Linux/0.1.23 (integration-test)"
)

func main() {
	user := os.Getenv("PROTON_USER")
	password := os.Getenv("PROTON_PASSWORD")
	totpKey := os.Getenv("PROTON_KEY")

	if user == "" || password == "" || totpKey == "" {
		fmt.Fprintln(os.Stderr,
			"PROTON_USER, PROTON_PASSWORD, and PROTON_KEY must all be set")
		os.Exit(1)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
	defer cancel()

	// Generate the current TOTP code.
	totpCode, err := generateTOTP(totpKey)
	if err != nil {
		fatalf("TOTP generation failed: %v", err)
	}
	fmt.Printf("[test] TOTP code generated (first 3 digits: %s…)\n", totpCode[:3])

	// Attempt to log in to Proton Drive.
	drive, err := loginWithRetry(ctx, user, password, totpCode)
	if err != nil {
		fatalf("login failed: %v", err)
	}
	fmt.Println("[test] login successful")

	// List the root folder.
	rootLinkID := drive.RootLink.LinkID
	fmt.Printf("[test] root link ID: %s\n", rootLinkID)

	items, err := drive.ListDirectory(ctx, rootLinkID)
	if err != nil {
		fatalf("ListDirectory failed: %v", err)
	}

	fmt.Printf("[test] root folder contains %d item(s)\n", len(items))
	if len(items) == 0 {
		fatalf("expected at least one item in root, got 0 — sync would be a no-op")
	}

	// Print the first 10 items.
	limit := 10
	if len(items) < limit {
		limit = len(items)
	}
	for i, item := range items[:limit] {
		kind := "file"
		if item.IsFolder {
			kind = "dir"
		}
		fmt.Printf("  [%d] %s (%s)\n", i+1, item.Name, kind)
	}
	if len(items) > 10 {
		fmt.Printf("  … and %d more\n", len(items)-10)
	}

	fmt.Println("[test] PASS")
}

// loginWithRetry attempts login and handles Human Verification (error 9001)
// by solving the captcha and retrying once.
func loginWithRetry(ctx context.Context, user, password, totpCode string) (*bridge.ProtonDrive, error) {
	cfg := newConfig(user, password, totpCode)

	// First attempt — straightforward credential login.
	drive, _, err := bridge.NewProtonDrive(ctx, cfg, nil, nil)
	if err == nil {
		return drive, nil
	}

	// Check whether Proton is asking for Human Verification (code 9001).
	hvToken, isHV := extractHVToken(err)
	if !isHV {
		return nil, fmt.Errorf("initial login: %w", err)
	}

	fmt.Printf("[test] HumanVerification challenge received (token: %s…)\n",
		hvToken[:minInt(12, len(hvToken))])

	// Solve the captcha.
	solver := newCaptchaSolver(hvToken)
	solvedToken, captchaErr := solver.Solve(ctx)
	if captchaErr != nil {
		return nil, fmt.Errorf("captcha solve: %w", captchaErr)
	}
	fmt.Printf("[test] captcha solved, solution token: %s…\n",
		solvedToken[:minInt(16, len(solvedToken))])

	// Re-login manually with HV headers, mirroring pd_login_hv in bridge.go.
	return loginWithHV(ctx, user, password, totpCode, solvedToken)
}

// loginWithHV re-authenticates after solving the CAPTCHA, injecting the
// HV solution token as HTTP headers on every request.  This mirrors the
// pd_login_hv implementation in bridge.go exactly.
func loginWithHV(ctx context.Context, user, password, totpCode, solvedToken string) (*bridge.ProtonDrive, error) {
	// 1. Build a fresh manager with HV headers.
	m := proton.New(
		proton.WithAppVersion(appVersion),
		proton.WithUserAgent(userAgent),
	)
	m.AddPreRequestHook(func(_ *resty.Client, req *resty.Request) error {
		req.SetHeader("x-pm-human-verification-token", solvedToken)
		req.SetHeader("x-pm-human-verification-token-type", "captcha")
		return nil
	})

	// 2. Authenticate with SRP.
	c, auth, err := m.NewClientWithLogin(ctx, user, []byte(password))
	if err != nil {
		return nil, fmt.Errorf("hv login: %w", err)
	}

	// 3. 2FA.
	if auth.TwoFA.Enabled&proton.HasTOTP != 0 {
		if totpCode == "" {
			return nil, fmt.Errorf("2FA required but no TOTP code provided")
		}
		if err := c.Auth2FA(ctx, proton.Auth2FAReq{TwoFactorCode: totpCode}); err != nil {
			return nil, fmt.Errorf("hv 2fa: %w", err)
		}
	}

	// 4. Compute salted key passphrase.
	user2, err := c.GetUser(ctx)
	if err != nil {
		return nil, fmt.Errorf("hv get user: %w", err)
	}
	salts, err := c.GetSalts(ctx)
	if err != nil {
		return nil, fmt.Errorf("hv get salts: %w", err)
	}
	saltedKeyPass, err := salts.SaltForKey([]byte(password), user2.Keys.Primary().ID)
	if err != nil {
		return nil, fmt.Errorf("hv salt key: %w", err)
	}

	// 5. Switch to reusable-login mode with the obtained credential.
	cfg := &common.Config{
		AppVersion: appVersion,
		UserAgent:  userAgent,
		FirstLoginCredential: &common.FirstLoginCredentialData{
			Username: user,
		},
		ReusableCredential: &common.ReusableCredentialData{
			UID:           auth.UID,
			AccessToken:   auth.AccessToken,
			RefreshToken:  auth.RefreshToken,
			SaltedKeyPass: base64.StdEncoding.EncodeToString(saltedKeyPass),
		},
		UseReusableLogin: true,
	}

	drive, _, err := bridge.NewProtonDrive(ctx, cfg, nil, nil)
	if err != nil {
		return nil, fmt.Errorf("hv drive init: %w", err)
	}
	return drive, nil
}

// newConfig builds a common.Config for first-time login.
func newConfig(user, password, totpCode string) *common.Config {
	return &common.Config{
		AppVersion: appVersion,
		UserAgent:  userAgent,
		FirstLoginCredential: &common.FirstLoginCredentialData{
			Username: user,
			Password: password,
			TwoFA:    totpCode,
		},
		ReusableCredential: &common.ReusableCredentialData{},
		UseReusableLogin:   false,
	}
}

// extractHVToken checks whether err is a Proton API 9001
// HumanVerification error and returns the challenge token.
// Mirrors extractHVDetails in bridge.go exactly.
func extractHVToken(err error) (string, bool) {
	if err == nil {
		return "", false
	}

	var apiErr *proton.APIError
	if errors.As(err, &apiErr) && apiErr.Code == proton.HumanVerificationRequired {
		if detailsMap, ok := apiErr.Details.(map[string]interface{}); ok {
			if tok, ok := detailsMap["HumanVerificationToken"].(string); ok && tok != "" {
				return tok, true
			}
		}
	}
	return "", false
}

// ─── helpers ──────────────────────────────────────────────────────────────

func fatalf(format string, args ...interface{}) {
	fmt.Fprintf(os.Stderr, "[FAIL] "+format+"\n", args...)
	os.Exit(1)
}

func minInt(a, b int) int {
	if a < b {
		return a
	}
	return b
}
