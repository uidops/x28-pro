// Renders the 1024 px app icon: a soft charcoal "X28" wordmark on light grey,
// matching the app's black-and-white scheme, with a gentle top glow, laid out on the macOS icon grid
// (824 px body, 100 px margin for the drop shadow).
//
//   swift packaging/macos/icon.swift out.png

import AppKit

let output = URL(fileURLWithPath: CommandLine.arguments[1])

func color(_ hex: UInt32) -> NSColor {
    NSColor(
        srgbRed: CGFloat((hex >> 16) & 0xff) / 255,
        green: CGFloat((hex >> 8) & 0xff) / 255,
        blue: CGFloat(hex & 0xff) / 255,
        alpha: 1
    )
}

let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: 1024, pixelsHigh: 1024,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)

let body = NSBezierPath(
    roundedRect: NSRect(x: 100, y: 100, width: 824, height: 824),
    xRadius: 185, yRadius: 185
)

// Diffuse, light drop shadow under the body.
NSGraphicsContext.saveGraphicsState()
let shadow = NSShadow()
shadow.shadowColor = NSColor.black.withAlphaComponent(0.2)
shadow.shadowBlurRadius = 40
shadow.shadowOffset = NSSize(width: 0, height: -14)
shadow.set()
color(0xd4d4d4).setFill()
body.fill()
NSGraphicsContext.restoreGraphicsState()

// Light grey, lighter at the top, kept low in contrast.
NSGradient(colors: [color(0xfbfbfb), color(0xebebeb), color(0xd2d2d2)])!.draw(in: body, angle: -90)

// Soft glow from the top, like light on a rounded surface.
NSGraphicsContext.saveGraphicsState()
body.addClip()
NSGradient(starting: NSColor.white.withAlphaComponent(0.45), ending: NSColor.white.withAlphaComponent(0))!
    .draw(fromCenter: NSPoint(x: 512, y: 940), radius: 0, toCenter: NSPoint(x: 512, y: 940), radius: 620, options: [])
NSGraphicsContext.restoreGraphicsState()

// Faint light rim along the edge.
NSColor.white.withAlphaComponent(0.6).setStroke()
body.lineWidth = 3
body.stroke()

// "X28" in SF Pro Rounded Bold, charcoal with a soft shadow, centered on its
// cap height; the X a lighter grey so the wordmark reads gently.
let base = NSFont.systemFont(ofSize: 320, weight: .bold)
let font = base.fontDescriptor.withDesign(.rounded).flatMap { NSFont(descriptor: $0, size: 320) } ?? base
let textShadow = NSShadow()
textShadow.shadowColor = NSColor.black.withAlphaComponent(0.18)
textShadow.shadowBlurRadius = 24
textShadow.shadowOffset = NSSize(width: 0, height: -8)
let word = NSMutableAttributedString(string: "X28", attributes: [
    .font: font,
    .foregroundColor: color(0x1f1f1f),
    .kern: -6,
    .shadow: textShadow,
])
word.addAttribute(.foregroundColor, value: color(0x6b6b6b), range: NSRange(location: 0, length: 1))
let wordSize = word.size()
// draw(at:) takes the bottom-left of the line box; put the baseline so the
// caps sit on the vertical center.
let baseline = 512 - font.capHeight / 2
word.draw(at: NSPoint(x: (1024 - wordSize.width) / 2, y: baseline + font.descender))

NSGraphicsContext.restoreGraphicsState()
try rep.representation(using: .png, properties: [:])!.write(to: output)
