// Dump the macOS accessibility tree of the running UCompute app (what VoiceOver reads: role, label, value,
// help, focus). A VoiceOver-level check without VoiceOver; it is not a test with VoiceOver itself.
// The calling process must already be trusted for Accessibility (this never asks).
//   swift scripts/macos-ax-dump.swift [process name, default ucompute-desktop] [max depth, default 40] [link] > out.txt
// With [link], first presses the window's link with that title (e.g. a sidebar section such as "Devices", the
// same AXPress VoiceOver's VO-Space sends) and waits a second for the view to render.
import ApplicationServices
import Foundation

let args = CommandLine.arguments
let name = args.count > 1 ? args[1] : "ucompute-desktop"
let maxDepth = args.count > 2 ? Int(args[2]) ?? 40 : 40
let press = args.count > 3 ? args[3] : nil
guard AXIsProcessTrusted() else { FileHandle.standardError.write("not trusted for Accessibility\n".data(using: .utf8)!); exit(2) }
let pgrep = Process()
pgrep.executableURL = URL(fileURLWithPath: "/usr/bin/pgrep")
pgrep.arguments = ["-x", name]
let pipe = Pipe()
pgrep.standardOutput = pipe
try pgrep.run()
pgrep.waitUntilExit()
guard let pid = Int32(String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    .split(separator: "\n").first ?? "") else { print("no process \(name)"); exit(1) }

func attr(_ e: AXUIElement, _ a: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, a as CFString, &v) == .success ? v : nil
}
func str(_ e: AXUIElement, _ a: String) -> String {
    guard let v = attr(e, a) else { return "" }
    if let s = v as? String { return s }
    if let n = v as? NSNumber { return n.stringValue }
    return ""
}
var count = 0
func walk(_ e: AXUIElement, _ d: Int) {
    guard d <= maxDepth else { return }
    count += 1
    var parts = [str(e, kAXRoleAttribute)]
    let sub = str(e, kAXSubroleAttribute)
    if !sub.isEmpty { parts.append("(\(sub))") }
    for (k, a) in [("title", kAXTitleAttribute), ("label", kAXDescriptionAttribute), ("value", kAXValueAttribute),
                   ("help", kAXHelpAttribute), ("roledesc", kAXRoleDescriptionAttribute)] {
        let v = str(e, a).replacingOccurrences(of: "\n", with: " ")
        if !v.isEmpty { parts.append("\(k)=\"\(v.prefix(160))\"") }
    }
    if str(e, kAXFocusedAttribute) == "1" { parts.append("FOCUSED") }
    if str(e, kAXEnabledAttribute) == "0" { parts.append("disabled") }
    print(String(repeating: "  ", count: d) + parts.joined(separator: " "))
    for c in (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] { walk(c, d + 1) }
}
func find(_ e: AXUIElement, role: String, title: String, _ d: Int = 0) -> AXUIElement? {
    if d > maxDepth { return nil }
    if str(e, kAXRoleAttribute) == role && str(e, kAXTitleAttribute) == title { return e }
    for c in (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] {
        if let hit = find(c, role: role, title: title, d + 1) { return hit }
    }
    return nil
}
let app = AXUIElementCreateApplication(pid)
let windows = (attr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? []
if let press {
    guard let link = windows.lazy.compactMap({ find($0, role: "AXLink", title: press) }).first,
          AXUIElementPerformAction(link, kAXPressAction as CFString) == .success
    else { print("no link \(press)"); exit(1) }
    Thread.sleep(forTimeInterval: 1.0)
}
for w in (attr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? [] { walk(w, 0) }
print("AX_DONE \(count) elements")
