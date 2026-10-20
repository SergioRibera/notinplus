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
                publish(url, source: "launch")
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
            publish(url, source: "open-url")
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

// Starts the runtime, calls the generated `IstmoPluginRegistry` and
// jumps into the Rust `#[istmo::mobile_app]` entry point. The
// registry auto-registers `data-store` and `file-picker`; `istmo-pen`
// opts out and is wired inside the Rust side of the app.
DeepLinks.install()
IstmoApp.run()
