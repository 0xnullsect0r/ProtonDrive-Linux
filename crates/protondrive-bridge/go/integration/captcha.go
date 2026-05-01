package main

// Proton drag-puzzle CAPTCHA solver.
//
// Flow:
//   1. Open the captcha challenge page in headless Chrome.
//   2. Download the background and piece images directly (faster than screenshot).
//   3. Run NCC template-matching to find the slot centre.
//   4. Drag from the piece's starting position to the slot along a Bézier
//      path, adding per-step random jitter and speed variance.
//   5. Intercept the Proton verify API response to capture the solution token,
//      then fall back to DOM polling.

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"image"
	_ "image/jpeg"
	_ "image/png"
	"io"
	"math"
	"math/rand"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/chromedp/cdproto/input"
	"github.com/chromedp/cdproto/network"
	"github.com/chromedp/chromedp"
)

const (
	// verifyProtonMe is the base URL for Proton's captcha service.
	verifyProtonMe = "https://verify.proton.me"
)

// CaptchaSolver holds state for a single captcha challenge.
type CaptchaSolver struct {
	hvToken string
	rng     *rand.Rand
}

func newCaptchaSolver(hvToken string) *CaptchaSolver {
	return &CaptchaSolver{
		hvToken: hvToken,
		rng:     rand.New(rand.NewSource(time.Now().UnixNano())),
	}
}

// Solve opens the captcha page, finds the slot, drags the piece, and
// returns the verification token that Proton accepts.
func (s *CaptchaSolver) Solve(ctx context.Context) (string, error) {
	// ── 1. Build a headless Chrome context ──────────────────────────────
	allocOpts := append(chromedp.DefaultExecAllocatorOptions[:],
		chromedp.Flag("headless", true),
		chromedp.Flag("no-sandbox", true),
		chromedp.Flag("disable-dev-shm-usage", true),
		chromedp.Flag("disable-gpu", true),
		chromedp.Flag("window-size", "1280,800"),
		chromedp.UserAgent(
			"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "+
				"(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"),
	)

	allocCtx, allocCancel := chromedp.NewExecAllocator(ctx, allocOpts...)
	defer allocCancel()

	taskCtx, taskCancel := chromedp.NewContext(allocCtx)
	defer taskCancel()

	// ── 2. Set up network interception to capture the solution token ─────
	var (
		solveTokenMu sync.Mutex
		solveToken   string
	)
	captureResponseBodies := map[network.RequestID]struct{}{}
	var captureBodyMu sync.Mutex

	chromedp.ListenTarget(taskCtx, func(ev interface{}) {
		switch e := ev.(type) {
		case *network.EventResponseReceived:
			url := e.Response.URL
			if strings.Contains(url, "verify") || strings.Contains(url, "captcha") {
				captureBodyMu.Lock()
				captureResponseBodies[e.RequestID] = struct{}{}
				captureBodyMu.Unlock()
			}
		case *network.EventLoadingFinished:
			captureBodyMu.Lock()
			_, wanted := captureResponseBodies[e.RequestID]
			captureBodyMu.Unlock()
			if !wanted {
				return
			}
			// Retrieve the body asynchronously.
			go func(rid network.RequestID) {
				body, err := network.GetResponseBody(rid).Do(taskCtx)
				if err != nil || len(body) == 0 {
					return
				}
				tok := extractTokenFromBody(body)
				if tok == "" {
					return
				}
				solveTokenMu.Lock()
				if solveToken == "" {
					solveToken = tok
				}
				solveTokenMu.Unlock()
			}(e.RequestID)
		}
	})

	// ── 3. Load the captcha page ─────────────────────────────────────────
	captchaURL := fmt.Sprintf(
		"%s/captcha/v1/html5.html?token=%s&mode=embedded",
		verifyProtonMe, s.hvToken)

	if err := chromedp.Run(taskCtx,
		network.Enable(),
		chromedp.Navigate(captchaURL),
		chromedp.Sleep(2500*time.Millisecond),
	); err != nil {
		return "", fmt.Errorf("navigate captcha page: %w", err)
	}

	// ── 4. Download background + piece images for template matching ───────
	bg, piece, err := s.fetchImages()
	if err != nil {
		// Fall back: take a screenshot and try to infer positions
		return s.solveFromScreenshot(taskCtx, &solveToken, &solveTokenMu)
	}

	// ── 5. Find the slot via NCC ──────────────────────────────────────────
	slotCX, slotCY := findSlotPosition(bg, piece)
	fmt.Printf("[captcha] slot centre estimated at (%d, %d)\n", slotCX, slotCY)

	// ── 6. Find the puzzle-piece starting position via DOM ────────────────
	var pieceRect map[string]interface{}
	if err := chromedp.Run(taskCtx, chromedp.Evaluate(`
		(function(){
			var sel = [
				'[draggable="true"]',
				'[class*="piece"]', '[class*="slider"]',
				'[class*="puzzle"]', '[id*="piece"]',
			];
			for (var i = 0; i < sel.length; i++) {
				var el = document.querySelector(sel[i]);
				if (el) {
					var r = el.getBoundingClientRect();
					return {x: r.left + r.width/2, y: r.top + r.height/2, found: true};
				}
			}
			// Fallback: piece starts at left edge of the captcha area
			var root = document.querySelector('body');
			var r = root.getBoundingClientRect();
			return {x: r.left + 30, y: r.top + r.height/2, found: false};
		})()
	`, &pieceRect)); err != nil {
		return "", fmt.Errorf("query piece position: %w", err)
	}

	startX := mapToFloat(pieceRect["x"])
	startY := mapToFloat(pieceRect["y"])

	// The slot coordinates from template-matching are relative to the
	// background image origin.  Map them to page coordinates using the
	// background element's position in the DOM.
	var bgRect map[string]interface{}
	if err := chromedp.Run(taskCtx, chromedp.Evaluate(`
		(function(){
			var sel = [
				'canvas', 'img[class*="bg"]', '[class*="background"]',
				'[class*="track"]', '[class*="puzzle"]',
			];
			for (var i = 0; i < sel.length; i++) {
				var el = document.querySelector(sel[i]);
				if (el) {
					var r = el.getBoundingClientRect();
					return {left: r.left, top: r.top, found: true};
				}
			}
			return {left: 0, top: 0, found: false};
		})()
	`, &bgRect)); err != nil {
		return "", fmt.Errorf("query bg position: %w", err)
	}

	bgLeft := mapToFloat(bgRect["left"])
	bgTop := mapToFloat(bgRect["top"])

	endX := bgLeft + float64(slotCX)
	endY := bgTop + float64(slotCY)

	fmt.Printf("[captcha] dragging (%.0f,%.0f) → (%.0f,%.0f)\n",
		startX, startY, endX, endY)

	// ── 7. Simulate drag with Bézier path + random jitter + speed variance
	if err := s.simulateDrag(taskCtx, startX, startY, endX, endY); err != nil {
		return "", fmt.Errorf("simulate drag: %w", err)
	}

	// ── 8. Wait for the token (network interception or DOM poll) ──────────
	return s.waitForToken(taskCtx, &solveToken, &solveTokenMu, 8*time.Second)
}

// solveFromScreenshot is the fallback when image downloads fail.
// It takes a screenshot, estimates the slot as the darkest region in the
// right half, then drags from the left edge to that region.
func (s *CaptchaSolver) solveFromScreenshot(
	ctx context.Context,
	solveToken *string, mu *sync.Mutex,
) (string, error) {
	var buf []byte
	if err := chromedp.Run(ctx, chromedp.FullScreenshot(&buf, 90)); err != nil {
		return "", fmt.Errorf("screenshot fallback: %w", err)
	}

	img, _, err := image.Decode(bytes.NewReader(buf))
	if err != nil {
		return "", fmt.Errorf("decode screenshot: %w", err)
	}
	b := img.Bounds()
	w, h := b.Dx(), b.Dy()

	// Locate the darkest 50×50 tile in the right 70% of the screenshot —
	// that is most likely the slot shadow.
	tileW, tileH := 50, 50
	startTileX := w * 3 / 10
	bestDark, bestTX, bestTY := math.MaxFloat64, startTileX+tileW/2, h/2
	for ty := 0; ty <= h-tileH; ty += 4 {
		for tx := startTileX; tx <= w-tileW; tx += 4 {
			var sum float64
			for y := 0; y < tileH; y++ {
				for x := 0; x < tileW; x++ {
					r, g, bl, _ := img.At(tx+x, ty+y).RGBA()
					sum += 0.299*float64(r>>8) +
						0.587*float64(g>>8) +
						0.114*float64(bl>>8)
				}
			}
			if sum < bestDark {
				bestDark = sum
				bestTX = tx + tileW/2
				bestTY = ty + tileH/2
			}
		}
	}

	startX := float64(w/10 + 25)
	startY := float64(bestTY)
	endX := float64(bestTX)
	endY := float64(bestTY)

	fmt.Printf("[captcha][screenshot] dragging (%.0f,%.0f) → (%.0f,%.0f)\n",
		startX, startY, endX, endY)

	if err := s.simulateDrag(ctx, startX, startY, endX, endY); err != nil {
		return "", fmt.Errorf("simulate drag (screenshot path): %w", err)
	}
	return s.waitForToken(ctx, solveToken, mu, 8*time.Second)
}

// fetchImages downloads the background and piece PNG images from the
// Proton captcha service using the stored challenge token.
func (s *CaptchaSolver) fetchImages() (bg, piece image.Image, err error) {
	bgURL := fmt.Sprintf("%s/captcha/v1/png?Token=%s&Type=background",
		verifyProtonMe, s.hvToken)
	pieceURL := fmt.Sprintf("%s/captcha/v1/png?Token=%s&Type=piece",
		verifyProtonMe, s.hvToken)

	bg, err = fetchImage(bgURL)
	if err != nil {
		return nil, nil, fmt.Errorf("fetch background: %w", err)
	}
	piece, err = fetchImage(pieceURL)
	if err != nil {
		return nil, nil, fmt.Errorf("fetch piece: %w", err)
	}
	return bg, piece, nil
}

func fetchImage(url string) (image.Image, error) {
	resp, err := http.Get(url) //nolint:gosec
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("HTTP %d from %s", resp.StatusCode, url)
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	img, _, err := image.Decode(bytes.NewReader(body))
	return img, err
}

// ─── Drag simulation ───────────────────────────────────────────────────────

type pt struct{ x, y float64 }

// simulateDrag performs a realistic mouse drag from (sx,sy) to (ex,ey):
//   - Quadratic Bézier curve path with a randomised control point.
//   - Per-step jitter: ±3 px horizontal, ±2 px vertical.
//   - Occasional micro-pause or tiny backward wobble.
//   - Ease-in-out speed profile with ±30 % random variance per step.
func (s *CaptchaSolver) simulateDrag(ctx context.Context, sx, sy, ex, ey float64) error {
	const steps = 45

	waypoints := s.bezierWaypoints(sx, sy, ex, ey, steps)

	// Press at the starting position.
	if err := chromedp.Run(ctx, chromedp.ActionFunc(func(ctx context.Context) error {
		return input.DispatchMouseEvent(input.MousePressed, sx, sy).
			WithButton(input.Left).WithClickCount(1).Do(ctx)
	})); err != nil {
		return err
	}
	time.Sleep(s.jitterDuration(60, 120))

	for i, wp := range waypoints {
		// Per-step jitter — slightly more horizontal than vertical to mimic
		// a natural left-to-right drag without much vertical drift.
		jx := wp.x + s.rng.Float64()*6 - 3
		jy := wp.y + s.rng.Float64()*4 - 2

		// Clamp so we don't leave the viewport.
		if jx < 0 {
			jx = 0
		}
		if jy < 0 {
			jy = 0
		}

		cx, cy := jx, jy // capture for the closure
		if err := chromedp.Run(ctx, chromedp.ActionFunc(func(ctx context.Context) error {
			return input.DispatchMouseEvent(input.MouseMoved, cx, cy).
				WithButton(input.Left).Do(ctx)
		})); err != nil {
			return err
		}

		// Ease-in-out speed: slow at start and end, fast in the middle.
		t := float64(i) / float64(steps-1)
		eased := easeInOut(t) // 0 → 1
		// Map to delay: 25 ms at extremes, 8 ms at centre.
		baseDelay := 25 - 17*eased // ms
		delay := baseDelay * (0.7 + s.rng.Float64()*0.6)
		time.Sleep(time.Duration(delay * float64(time.Millisecond)))

		// Occasional micro-wobble: a tiny backward nudge (~5 % of steps)
		// to simulate human uncertainty.
		if s.rng.Intn(20) == 0 && i > 2 && i < steps-3 {
			wobbleX := jx - s.rng.Float64()*4
			wbx := wobbleX
			_ = chromedp.Run(ctx, chromedp.ActionFunc(func(ctx context.Context) error {
				return input.DispatchMouseEvent(input.MouseMoved, wbx, jy).
					WithButton(input.Left).Do(ctx)
			}))
			time.Sleep(s.jitterDuration(15, 30))
		}
	}

	// Brief pause before release, as a human would.
	time.Sleep(s.jitterDuration(80, 160))

	// Release at the exact target to maximise tolerance matching.
	return chromedp.Run(ctx, chromedp.ActionFunc(func(ctx context.Context) error {
		return input.DispatchMouseEvent(input.MouseReleased, ex, ey).
			WithButton(input.Left).WithClickCount(1).Do(ctx)
	}))
}

// bezierWaypoints returns `steps` points along a quadratic Bézier curve
// from (x1,y1) to (x2,y2) with a randomised control point offset.
func (s *CaptchaSolver) bezierWaypoints(x1, y1, x2, y2 float64, steps int) []pt {
	// Control point: slightly above/below the midpoint for a natural arc.
	midX := (x1 + x2) / 2
	midY := (y1 + y2) / 2
	cpX := midX + s.rng.Float64()*16 - 8
	cpY := midY + s.rng.Float64()*20 - 10

	pts := make([]pt, steps)
	for i := range pts {
		t := float64(i) / float64(steps-1)
		// Quadratic Bézier: B(t) = (1-t)²P0 + 2(1-t)t·CP + t²P1
		inv := 1 - t
		bx := inv*inv*x1 + 2*inv*t*cpX + t*t*x2
		by := inv*inv*y1 + 2*inv*t*cpY + t*t*y2
		pts[i] = pt{bx, by}
	}
	return pts
}

// easeInOut returns a smooth 0→1 value for t∈[0,1].
func easeInOut(t float64) float64 {
	return t * t * (3 - 2*t) // smoothstep
}

// jitterDuration returns a random duration in [minMs, maxMs] milliseconds.
func (s *CaptchaSolver) jitterDuration(minMs, maxMs int) time.Duration {
	ms := minMs + s.rng.Intn(maxMs-minMs+1)
	return time.Duration(ms) * time.Millisecond
}

// ─── Token extraction ──────────────────────────────────────────────────────

// waitForToken waits up to `timeout` for the solve token to appear, polling
// network-interception state and the DOM alternately.
func (s *CaptchaSolver) waitForToken(
	ctx context.Context,
	solveToken *string, mu *sync.Mutex,
	timeout time.Duration,
) (string, error) {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		mu.Lock()
		tok := *solveToken
		mu.Unlock()
		if tok != "" {
			fmt.Printf("[captcha] token captured via network: %s…\n", tok[:min(16, len(tok))])
			return tok, nil
		}

		// DOM poll: check common places the page might store the token.
		var domToken string
		_ = chromedp.Run(ctx, chromedp.Evaluate(`
			(function(){
				return window.__protonCaptchaToken
					|| window.challengeToken
					|| window.__token
					|| (document.querySelector('[data-token]') &&
					    document.querySelector('[data-token]').getAttribute('data-token'))
					|| (document.querySelector('input[name="token"]') &&
					    document.querySelector('input[name="token"]').value)
					|| '';
			})()
		`, &domToken))
		if domToken != "" {
			fmt.Printf("[captcha] token captured via DOM: %s…\n", domToken[:min(16, len(domToken))])
			return domToken, nil
		}

		time.Sleep(400 * time.Millisecond)
	}

	return "", fmt.Errorf("captcha token not received within %s", timeout)
}

// extractTokenFromBody attempts to pull a verification token from a raw
// JSON response body.  Proton typically returns {"Token":"…","Code":1000}.
func extractTokenFromBody(body []byte) string {
	var payload struct {
		Token string `json:"Token"`
		Code  int    `json:"Code"`
	}
	if err := json.Unmarshal(body, &payload); err == nil && payload.Token != "" {
		return payload.Token
	}
	// Fallback: look for a "token" key in any JSON object.
	var generic map[string]interface{}
	if err := json.Unmarshal(body, &generic); err == nil {
		for k, v := range generic {
			if strings.EqualFold(k, "token") {
				if s, ok := v.(string); ok && s != "" {
					return s
				}
			}
		}
	}
	return ""
}

// mapToFloat safely converts interface{} (JSON number) to float64.
func mapToFloat(v interface{}) float64 {
	if f, ok := v.(float64); ok {
		return f
	}
	return 0
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}
