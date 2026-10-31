// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Draws Peabody's icon: a speech bubble holding the rhythm of a voice. Writes
// brand/app-icon/icon.png (1024 px), from which `cargo tauri icon` makes the app's icons.
// Run: swift scripts/make-icon.swift && cargo tauri icon brand/app-icon/icon.png -o crates/agent-peabody-app/icons

import AppKit

func draw(size s: CGFloat) -> NSBitmapImageRep {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(s), pixelsHigh: Int(s), bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let inset = s * 0.1
    let tile = NSRect(x: inset, y: inset, width: s - 2 * inset, height: s - 2 * inset)
    let shape = NSBezierPath(roundedRect: tile, xRadius: s * 0.18, yRadius: s * 0.18)
    NSGradient(colors: [NSColor(srgbRed: 0.45, green: 0.20, blue: 0.40, alpha: 1),
                        NSColor(srgbRed: 0.22, green: 0.16, blue: 0.45, alpha: 1)])!.draw(in: shape, angle: -90)

    // The bubble, with its tail at the lower left.
    NSColor(white: 1, alpha: 0.95).setFill()
    let bubble = NSRect(x: s * 0.2, y: s * 0.34, width: s * 0.6, height: s * 0.42)
    NSBezierPath(roundedRect: bubble, xRadius: s * 0.12, yRadius: s * 0.12).fill()
    let tail = NSBezierPath()
    tail.move(to: NSPoint(x: s * 0.30, y: s * 0.36))
    tail.line(to: NSPoint(x: s * 0.26, y: s * 0.22))
    tail.line(to: NSPoint(x: s * 0.44, y: s * 0.36))
    tail.close()
    tail.fill()

    // Inside it, a voice's rhythm.
    NSColor(srgbRed: 0.30, green: 0.18, blue: 0.43, alpha: 1).setFill()
    let bars: [CGFloat] = [0.08, 0.18, 0.26, 0.16, 0.22, 0.10]
    let barWidth = s * 0.04
    let step = barWidth * 1.9
    let first = bubble.midX - (CGFloat(bars.count) * step - (step - barWidth)) / 2
    for (i, h) in bars.enumerated() {
        let rect = NSRect(x: first + CGFloat(i) * step, y: bubble.midY - s * h / 2, width: barWidth, height: s * h)
        NSBezierPath(roundedRect: rect, xRadius: barWidth / 2, yRadius: barWidth / 2).fill()
    }
    NSGraphicsContext.restoreGraphicsState()
    return rep
}

let out = URL(fileURLWithPath: "brand/app-icon/icon.png")
try FileManager.default.createDirectory(at: out.deletingLastPathComponent(), withIntermediateDirectories: true)
try draw(size: 1024).representation(using: .png, properties: [:])!.write(to: out)
print("wrote \(out.path)")
