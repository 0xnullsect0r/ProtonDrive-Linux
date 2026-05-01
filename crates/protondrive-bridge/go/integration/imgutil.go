package main

// Image analysis utilities for the drag-puzzle CAPTCHA solver.
//
// findSlotPosition uses normalised cross-correlation (NCC) to locate the
// position in the background image that best matches the puzzle piece.
// The slot (hole) in the background was cut from the same image as the
// piece, so the piece template should correlate strongly with the region
// surrounding the hole — or in many implementations the hole itself is
// rendered as a bright border that matches the piece outline when
// inverted.  We try both the piece and its inverted form and pick the
// better scoring result.

import (
	"image"
	"image/color"
	"math"
)

// findSlotPosition returns the centre (cx, cy) of the best-match region
// in bg for the given piece template.  The coordinates are in the pixel
// space of bg.Bounds().
func findSlotPosition(bg, piece image.Image) (cx, cy int) {
	bgGray := toGray32(bg)
	pieceGray := toGray32(piece)
	bgB := bg.Bounds()
	pB := piece.Bounds()
	bgW, bgH := bgB.Dx(), bgB.Dy()
	pW, pH := pB.Dx(), pB.Dy()

	if pW >= bgW || pH >= bgH {
		// Piece is as large as (or larger than) the background; fall back
		// to the centre of the background.
		return bgW / 2, bgH / 2
	}

	// Also try the inverted piece: the slot shadow often inverts the
	// brightness relative to the piece surface.
	invPiece := invertGray(pieceGray, pW, pH)

	bx1, by1, s1 := nccBestMatch(bgGray, pieceGray, bgW, bgH, pW, pH)
	bx2, by2, s2 := nccBestMatch(bgGray, invPiece, bgW, bgH, pW, pH)

	if s1 >= s2 {
		return bx1 + pW/2, by1 + pH/2
	}
	return bx2 + pW/2, by2 + pH/2
}

// nccBestMatch slides piece across bg and returns the top-left offset
// (tx, ty) of the highest NCC score.
func nccBestMatch(bg, piece []float64, bgW, bgH, pW, pH int) (bestX, bestY int, bestScore float64) {
	// Precompute piece mean and denominator.
	var pieceSum float64
	for _, v := range piece {
		pieceSum += v
	}
	pieceMean := pieceSum / float64(pW*pH)

	var pieceDenom float64
	for _, v := range piece {
		d := v - pieceMean
		pieceDenom += d * d
	}
	pieceDenom = math.Sqrt(pieceDenom)

	bestScore = -math.MaxFloat64

	for ty := 0; ty <= bgH-pH; ty++ {
		for tx := 0; tx <= bgW-pW; tx++ {
			// Window mean.
			var winSum float64
			for y := 0; y < pH; y++ {
				for x := 0; x < pW; x++ {
					winSum += bg[(ty+y)*bgW+(tx+x)]
				}
			}
			winMean := winSum / float64(pW*pH)

			var ncc, winDenom float64
			for y := 0; y < pH; y++ {
				for x := 0; x < pW; x++ {
					pd := piece[y*pW+x] - pieceMean
					wd := bg[(ty+y)*bgW+(tx+x)] - winMean
					ncc += pd * wd
					winDenom += wd * wd
				}
			}
			winDenom = math.Sqrt(winDenom)

			if winDenom < 1e-10 || pieceDenom < 1e-10 {
				continue
			}
			score := ncc / (pieceDenom * winDenom)
			if score > bestScore {
				bestScore = score
				bestX, bestY = tx, ty
			}
		}
	}
	return
}

// toGray32 converts any image to a flat []float64 slice (row-major, [0,255]).
func toGray32(img image.Image) []float64 {
	b := img.Bounds()
	w, h := b.Dx(), b.Dy()
	out := make([]float64, w*h)
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			r, g, bl, _ := img.At(b.Min.X+x, b.Min.Y+y).RGBA()
			// RGBA() returns values in [0, 65535]; convert to [0, 255].
			lum := 0.299*float64(r>>8) + 0.587*float64(g>>8) + 0.114*float64(bl>>8)
			out[y*w+x] = lum
		}
	}
	return out
}

// invertGray returns 255 - v for every element.
func invertGray(src []float64, w, h int) []float64 {
	out := make([]float64, w*h)
	for i, v := range src {
		out[i] = 255 - v
	}
	return out
}

// graySliceToImage wraps a []float64 as an image.Gray for debugging.
func graySliceToImage(data []float64, w, h int) image.Image {
	img := image.NewGray(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.SetGray(x, y, color.Gray{Y: uint8(data[y*w+x])})
		}
	}
	return img
}
