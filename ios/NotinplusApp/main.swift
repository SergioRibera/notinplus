import CryptoKit
import Foundation
import IstmoRuntime
import ObjectiveC
import UIKit

/// `notein://` URLs reach the Rust side through the istmo deep-link
/// queue. winit installs its own application delegate, and UIKit
/// snapshots which delegate methods exist the moment the delegate is
/// set, so the URL handler is attached to the delegate's class from a
/// hook on `UIApplication.delegate`'s setter, before that snapshot.
enum DeepLinks {
    private static let channel = "istmo.deeplinks"
    private static let capacity: UInt32 = 16

    static func install() {
        hookDelegateSetter()
        NotificationCenter.default.addObserver(
            forName: UIApplication.didFinishLaunchingNotification,
            object: nil,
            queue: .main
        ) { note in
            if let url = note.userInfo?[UIApplication.LaunchOptionsKey.url] as? URL {
                if url.isFileURL {
                    OpenedDocuments.publish(url, origin: OpenedDocuments.originLaunch)
                } else {
                    publish(url, source: "launch")
                }
            }
        }
    }

    private static func hookDelegateSetter() {
        let setter = #selector(setter: UIApplication.delegate)
        guard let method = class_getInstanceMethod(UIApplication.self, setter) else { return }
        let original = method_getImplementation(method)
        typealias Setter = @convention(c) (AnyObject, Selector, AnyObject?) -> Void
        let callOriginal = unsafeBitCast(original, to: Setter.self)
        let replacement: @convention(block) (AnyObject, AnyObject?) -> Void = { app, delegate in
            if let delegate = delegate {
                addOpenURL(to: type(of: delegate))
            }
            callOriginal(app, setter, delegate)
        }
        method_setImplementation(method, imp_implementationWithBlock(replacement))
    }

    private static func addOpenURL(to cls: AnyClass) {
        let selector = #selector(UIApplicationDelegate.application(_:open:options:))
        guard class_getInstanceMethod(cls, selector) == nil else { return }
        let handler: @convention(block) (AnyObject, UIApplication, URL, NSDictionary) -> Bool = {
            _, _, url, _ in
            if url.isFileURL {
                OpenedDocuments.publish(url, origin: OpenedDocuments.originRunning)
            } else {
                publish(url, source: "open-url")
            }
            return true
        }
        class_addMethod(cls, selector, imp_implementationWithBlock(handler), "B@:@@@")
    }

    private static func publish(_ url: URL, source: String) {
        var out = Data()
        Bincode.writeString(&out, url.absoluteString)
        Bincode.writeOption(&out, source) { o, s in Bincode.writeString(&o, s) }
        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        Bincode.writeOption(&out, now) { o, ms in Bincode.writeVarintU64(&o, ms) }
        IstmoRuntime.shared.publishEarlyQueue(channel: channel, capacity: capacity, payload: out)
    }
}

/// Files opened with the app (Files › "Open in", the share sheet) go to
/// the Rust side on the `istmo.documents` queue as a plain path: each
/// one is copied into the temporary directory while its security scope
/// is open, to a folder named after its URL. The copy is what the app
/// opens; changes are not written back to the original, and a later
/// open of the same URL replaces it.
enum OpenedDocuments {
    private static let channel = "istmo.documents"
    private static let capacity: UInt32 = 32
    // Wire discriminants of `istmo_plugins::DocumentOrigin`.
    static let originLaunch: UInt32 = 0
    static let originRunning: UInt32 = 1
    // `istmo_plugins::DocumentLocation::Path`.
    private static let locationPath: UInt32 = 0

    static func publish(_ url: URL, origin: UInt32) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let name = url.lastPathComponent.isEmpty ? "document" : url.lastPathComponent
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("opened", isDirectory: true)
            .appendingPathComponent(folder(of: url), isDirectory: true)
        let copy = dir.appendingPathComponent(name)
        do {
            try? FileManager.default.removeItem(at: dir)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: url, to: copy)
        } catch {
            NSLog("OpenedDocuments: could not copy % %@", url.path, error.localizedDescription)
            return
        }
        var out = Data()
        Bincode.writeVarintU32(&out, locationPath)
        Bincode.writeString(&out, copy.path)
        Bincode.writeOption(&out, name) { o, s in Bincode.writeString(&o, s) }
        Bincode.writeOption(&out, nil as String?) { o, s in Bincode.writeString(&o, s) }
        Bincode.writeVarintU32(&out, origin)
        Bincode.writeOption(&out, nil as String?) { o, s in Bincode.writeString(&o, s) }
        IstmoRuntime.shared.publishEarlyQueue(channel: channel, capacity: capacity, payload: out)
    }

    /// One folder per source, so opening the same file again lands on the
    /// same path and the app sees the same file.
    private static func folder(of url: URL) -> String {
        SHA256.hash(data: Data(url.absoluteString.utf8))
            .prefix(16)
            .map { String(format: "%02x", $0) }
            .joined()
    }
}

// Starts the runtime, calls the generated `IstmoPluginRegistry` and
// jumps into the Rust `#[istmo::mobile_app]` entry point. The
// registry auto-registers `data-store` and `file-picker`; `istmo-pen`
// opts out and is wired inside the Rust side of the app.
DeepLinks.install()
IstmoApp.run()
