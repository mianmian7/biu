import Cocoa
import WebKit

final class Probe: NSObject, WKNavigationDelegate, NSApplicationDelegate {
    var web: WKWebView!
    var window: NSWindow!
    let url: URL
    var cases: [String] = []
    init(url: URL) { self.url = url }
    func applicationDidFinishLaunching(_ n: Notification) {
        let cfg = WKWebViewConfiguration()
        cfg.mediaTypesRequiringUserActionForPlayback = []
        web = WKWebView(frame: NSRect(x: 0, y: 0, width: 120, height: 120), configuration: cfg)
        web.navigationDelegate = self
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 120, height: 120), styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = web
        window.alphaValue = 0.05
        window.orderFrontRegardless()
        web.load(URLRequest(url: url))
        DispatchQueue.main.asyncAfter(deadline: .now() + 400) { print("TIMEOUT"); exit(2) }
    }
    func js(_ s: String, _ done: @escaping (Any?) -> Void) {
        web.evaluateJavaScript(s) { r, e in if let e = e { print("JSERR \(e)") }; done(r) }
    }
    func webView(_ w: WKWebView, didFinish n: WKNavigation!) {
        js("window.cases") { r in self.cases = r as? [String] ?? []; self.next(0) }
    }
    func next(_ i: Int) {
        guard i < cases.count else {
            js("navigator.userAgent") { ua in print("UA \(ua ?? "")"); exit(0) }
            return
        }
        let name = cases[i]
        js("window.start('\(name)')") { _ in
            var ticks = 0
            Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { t in
                ticks += 1
                if ticks >= 10 {
                    t.invalidate()
                    self.js("window.report('\(name)')") { r in print(r as? String ?? "nil"); fflush(stdout); self.next(i + 1) }
                } else {
                    self.js("window.state['\(name)'].a.currentTime") { _ in }
                }
            }
        }
    }
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let probe = Probe(url: URL(string: CommandLine.arguments[1])!)
app.delegate = probe
app.run()
