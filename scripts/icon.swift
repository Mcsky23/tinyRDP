import AppKit

// tinyRDP's connection mark: two open portals joined by a forward arrow.
// One geometry supplies the vector menu bar image and every app icon size.
func drawMark(in frame: NSRect, color: NSColor, arrowColor: NSColor? = nil) {
    let left = NSBezierPath()
    left.move(to: NSPoint(x: 6, y: 2.5))
    left.line(to: NSPoint(x: 4, y: 2.5))
    left.curve(to: NSPoint(x: 1.75, y: 4.75),
        controlPoint1: NSPoint(x: 2.75, y: 2.5), controlPoint2: NSPoint(x: 1.75, y: 3.5))
    left.line(to: NSPoint(x: 1.75, y: 13.25))
    left.curve(to: NSPoint(x: 4, y: 15.5),
        controlPoint1: NSPoint(x: 1.75, y: 14.5), controlPoint2: NSPoint(x: 2.75, y: 15.5))
    left.line(to: NSPoint(x: 6, y: 15.5))
    let right = NSBezierPath()
    right.move(to: NSPoint(x: 12, y: 2.5))
    right.line(to: NSPoint(x: 14, y: 2.5))
    right.curve(to: NSPoint(x: 16.25, y: 4.75),
        controlPoint1: NSPoint(x: 15.25, y: 2.5), controlPoint2: NSPoint(x: 16.25, y: 3.5))
    right.line(to: NSPoint(x: 16.25, y: 13.25))
    right.curve(to: NSPoint(x: 14, y: 15.5),
        controlPoint1: NSPoint(x: 16.25, y: 14.5), controlPoint2: NSPoint(x: 15.25, y: 15.5))
    right.line(to: NSPoint(x: 12, y: 15.5))
    let arrow = NSBezierPath()
    arrow.move(to: NSPoint(x: 5.75, y: 9))
    arrow.line(to: NSPoint(x: 12.25, y: 9))
    arrow.move(to: NSPoint(x: 9.75, y: 11.5))
    arrow.line(to: NSPoint(x: 12.25, y: 9))
    arrow.line(to: NSPoint(x: 9.75, y: 6.5))
    let transform = NSAffineTransform()
    transform.translateX(by: frame.minX, yBy: frame.minY)
    transform.scaleX(by: frame.width / 18, yBy: frame.height / 18)
    for (index, path) in [left, right, arrow].enumerated() {
        path.transform(using: transform as AffineTransform)
        path.lineWidth = 1.7 * frame.width / 18
        path.lineCapStyle = .round
        path.lineJoinStyle = .round
        (index == 2 ? (arrowColor ?? color) : color).setStroke()
        path.stroke()
    }
}

final class StatusIconView: NSView {
    override func draw(_ dirtyRect: NSRect) {
        drawMark(in: bounds, color: .black)
    }
}

let directory = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
if CommandLine.arguments.count > 2 {
    let status = StatusIconView(frame: NSRect(x: 0, y: 0, width: 18, height: 18))
    try status.dataWithPDF(inside: status.bounds).write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
}
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        let transform = NSAffineTransform()
        transform.scale(by: CGFloat(pixels) / 512)
        transform.concat()
        let tile = NSBezierPath(roundedRect: NSRect(x: 32, y: 32, width: 448, height: 448), xRadius: 104, yRadius: 104)
        NSGradient(starting: NSColor(srgbRed: 0.13, green: 0.36, blue: 0.59, alpha: 1),
            ending: NSColor(srgbRed: 0.08, green: 0.12, blue: 0.23, alpha: 1))!.draw(in: tile, angle: -70)
        drawMark(in: NSRect(x: 112, y: 112, width: 288, height: 288), color: .white,
            arrowColor: NSColor(srgbRed: 0.45, green: 0.91, blue: 0.83, alpha: 1))
        NSGraphicsContext.restoreGraphicsState()
        let name = "icon_\(size)x\(size)\(scale == 2 ? "@2x" : "").png"
        try bitmap.representation(using: .png, properties: [:])!.write(to: directory.appendingPathComponent(name))
    }
}
