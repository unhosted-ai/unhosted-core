// shotter — render a URL to PNG via WKWebView's takeSnapshot, headless.
//
// Why not screencapture: that needs Screen Recording permission and
// captures whatever happens to be on the screen. WKWebView renders
// through WebKit's own compositor; the resulting bitmap doesn't go
// through Window Server, so no permission is required and no
// unrelated content can sneak into the frame.
//
// Build:
//   swiftc -o scripts/shotter scripts/shotter.swift
// Run:
//   ./scripts/shotter <url> <out.png> [width=1280] [height=820] [predelay_ms=2500] [js="…"]
//
// The optional `js` argument runs after load + predelay (e.g. to open
// a sidebar section). takeSnapshot fires once that finishes.

import Cocoa
import WebKit

let args = CommandLine.arguments
guard args.count >= 3 else {
    FileHandle.standardError.write(Data("usage: shotter <url> <out.png> [w] [h] [predelay_ms] [js]\n".utf8))
    exit(2)
}
let url = URL(string: args[1])!
let outPath = args[2]
let width: CGFloat = args.count > 3 ? CGFloat(Double(args[3]) ?? 1280) : 1280
let height: CGFloat = args.count > 4 ? CGFloat(Double(args[4]) ?? 820) : 820
let predelayMs: Int = args.count > 5 ? (Int(args[5]) ?? 2500) : 2500
let preJs: String? = args.count > 6 ? args[6] : nil

final class Shotter: NSObject, WKNavigationDelegate {
    let outPath: String
    let preJs: String?
    let predelayMs: Int
    let web: WKWebView
    var finished = false

    init(outPath: String, preJs: String?, predelayMs: Int, web: WKWebView) {
        self.outPath = outPath
        self.preJs = preJs
        self.predelayMs = predelayMs
        self.web = web
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // First, give the page predelay_ms to settle (fonts, JS-driven
        // rendering, animations). Then optionally run user JS. Then
        // snapshot. Then exit.
        let deadline = DispatchTime.now() + .milliseconds(predelayMs)
        DispatchQueue.main.asyncAfter(deadline: deadline) { [self] in
            if let js = preJs, !js.isEmpty {
                webView.evaluateJavaScript(js) { [self] _, err in
                    if let err = err {
                        FileHandle.standardError.write(Data("js error: \(err)\n".utf8))
                    }
                    // Small extra delay after JS so the DOM mutation
                    // (e.g. expanding a <details>) re-renders.
                    DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(400)) { [self] in
                        self.snapshot()
                    }
                }
            } else {
                self.snapshot()
            }
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        FileHandle.standardError.write(Data("nav fail: \(error)\n".utf8))
        self.finished = true
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        FileHandle.standardError.write(Data("nav fail (provisional): \(error)\n".utf8))
        self.finished = true
    }

    func snapshot() {
        let config = WKSnapshotConfiguration()
        // Capture the full view, not just the visible region.
        config.afterScreenUpdates = true
        web.takeSnapshot(with: config) { [self] image, err in
            defer { self.finished = true }
            if let err = err {
                FileHandle.standardError.write(Data("snapshot: \(err)\n".utf8))
                return
            }
            guard let image = image else {
                FileHandle.standardError.write(Data("snapshot: no image\n".utf8))
                return
            }
            guard let tiff = image.tiffRepresentation,
                  let rep = NSBitmapImageRep(data: tiff),
                  let png = rep.representation(using: .png, properties: [:])
            else {
                FileHandle.standardError.write(Data("snapshot: encode failed\n".utf8))
                return
            }
            do {
                try png.write(to: URL(fileURLWithPath: self.outPath))
                print("[shotter] wrote \(self.outPath) (\(png.count) bytes)")
            } catch {
                FileHandle.standardError.write(Data("write: \(error)\n".utf8))
            }
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

// Offscreen window — required so layout runs at the desired size.
// WKWebView in an unattached NSView technically works, but pages
// that read window.innerWidth see 0 until the view is in a window.
let window = NSWindow(
    contentRect: NSRect(x: 0, y: 0, width: width, height: height),
    styleMask: [.borderless],
    backing: .buffered,
    defer: false
)
window.isReleasedWhenClosed = false
let cfg = WKWebViewConfiguration()
let web = WKWebView(frame: NSRect(x: 0, y: 0, width: width, height: height), configuration: cfg)
window.contentView = web
// Place way off-screen so we don't flicker the user's display, but
// still in a window so layout runs at full size.
window.setFrameOrigin(NSPoint(x: -10000, y: -10000))
window.orderFrontRegardless()

let shotter = Shotter(outPath: outPath, preJs: preJs, predelayMs: predelayMs, web: web)
web.navigationDelegate = shotter
web.load(URLRequest(url: url))

// Pump the runloop until snapshot finishes or we time out.
let timeoutAt = Date().addingTimeInterval(20)
while !shotter.finished && Date() < timeoutAt {
    RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05))
}
if !shotter.finished {
    FileHandle.standardError.write(Data("timeout after 20s\n".utf8))
    exit(3)
}
exit(0)
