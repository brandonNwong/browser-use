import AppKit
import Foundation

/// Paths the app owns: the shared secret, the in-process hub log, and the
/// repository README. There is no checkout/node/service discovery any more — the
/// hub lives in this process.
enum BrowserUsePaths {

    /// `~/.orange-mcp/secret`, shared with the Gecko Orange extension and CLI.
    static var secretFile: String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".orange-mcp/secret")
            .path
    }

    /// `secretFile` with the home directory abbreviated to `~`, for display in
    /// Settings and screenshots.
    static var displaySecretFile: String {
        (secretFile as NSString).abbreviatingWithTildeInPath
    }

    /// `~/Library/Logs/browser-use`, where the hub appends its log lines.
    static var logsDirectory: String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/browser-use")
            .path
    }

    /// The single in-process hub log passed to `bu_hub_start`.
    static var hubLog: String {
        URL(fileURLWithPath: logsDirectory).appendingPathComponent("hub.log").path
    }

    /// Creates `~/Library/Logs/browser-use` if needed; returns whether it exists.
    @discardableResult
    static func prepareLogsDirectory() -> Bool {
        do {
            try FileManager.default.createDirectory(
                atPath: logsDirectory,
                withIntermediateDirectories: true
            )
            return true
        } catch {
            NSLog("[%@] could not create %@: %@", AppInfo.name, logsDirectory, String(describing: error))
            return false
        }
    }

    /// Reveals the hub log in Finder.
    ///
    /// With a log file, select the file itself: Finder opens *inside*
    /// `~/Library/Logs/browser-use/` with `hub.log` pre-selected, so pressing
    /// Space Quick Looks it immediately. Without one, open the directory itself
    /// so Finder still lands inside `browser-use/` instead of selecting that
    /// folder from `Logs/` (which is what `activateFileViewerSelecting` on a
    /// directory would do).
    static func openLogs() {
        let fileManager = FileManager.default
        let logURL = URL(fileURLWithPath: hubLog)

        if fileManager.fileExists(atPath: logURL.path) {
            NSWorkspace.shared.activateFileViewerSelecting([logURL])
            return
        }

        let directory = URL(fileURLWithPath: logsDirectory)
        // The hub normally creates this at start-up; create it on demand so a
        // menu click in a dev/offline launch still lands in the right folder.
        try? fileManager.createDirectory(atPath: logsDirectory, withIntermediateDirectories: true)
        NSWorkspace.shared.open(directory)
    }

    /// `~/Library/Application Support/browser-use`, where `just install` (or
    /// `openExtensionFolder()`) stages the user-accessible Gecko Orange
    /// extension and `.xpi` archive.
    static var applicationSupportDirectory: String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/browser-use")
            .path
    }

    static var installedExtensionDirectory: String {
        URL(fileURLWithPath: applicationSupportDirectory)
            .appendingPathComponent("extension", isDirectory: true)
            .path
    }

    static var installedExtensionManifest: String {
        URL(fileURLWithPath: installedExtensionDirectory)
            .appendingPathComponent("manifest.json")
            .path
    }

    static var installedExtensionXpi: String {
        URL(fileURLWithPath: applicationSupportDirectory)
            .appendingPathComponent("gecko-orange.xpi")
            .path
    }

    static var bundledExtensionDirectory: URL? {
        Bundle.main.resourceURL?.appendingPathComponent("extension", isDirectory: true)
    }

    static var bundledExtensionXpi: URL? {
        Bundle.main.resourceURL?.appendingPathComponent("gecko-orange.xpi")
    }

    /// Walks up from the bundle looking for a source checkout's `extension/manifest.json`.
    static var checkoutExtensionManifest: String? {
        var directory = Bundle.main.bundleURL.deletingLastPathComponent()
        for _ in 0..<4 {
            let candidate = directory
                .appendingPathComponent("extension", isDirectory: true)
                .appendingPathComponent("manifest.json")
                .path
            if FileManager.default.fileExists(atPath: candidate) {
                return candidate
            }
            directory.deleteLastPathComponent()
        }
        return nil
    }

    /// Best available `manifest.json` path across source checkout, Application Support,
    /// and the embedded `.app/Contents/Resources/extension` copy.
    static var extensionManifestFile: String? {
        let fileManager = FileManager.default
        if let checkout = checkoutExtensionManifest {
            return checkout
        }
        if fileManager.fileExists(atPath: installedExtensionManifest) {
            return installedExtensionManifest
        }
        if let bundled = bundledExtensionDirectory?.appendingPathComponent("manifest.json").path,
           fileManager.fileExists(atPath: bundled) {
            return bundled
        }
        return nil
    }

    /// Reveals `extension/manifest.json` in Finder so the user can load or drag it
    /// into Zen's `about:debugging` → **Load Temporary Add-on…** dialog.
    ///
    /// When running from an installed `.app` bundle (for example dragged from a
    /// release zip), stages the bundled extension into
    /// `~/Library/Application Support/browser-use/` because macOS file-open panels
    /// treat `.app` bundles as opaque packages.
    static func openExtensionFolder() {
        let fileManager = FileManager.default
        if let checkout = checkoutExtensionManifest {
            reveal(checkout)
            return
        }
        if let bundledDir = bundledExtensionDirectory,
           fileManager.fileExists(atPath: bundledDir.appendingPathComponent("manifest.json").path) {
            syncBundledExtensionToApplicationSupport()
            if fileManager.fileExists(atPath: installedExtensionManifest) {
                reveal(installedExtensionManifest)
                return
            }
            reveal(bundledDir.appendingPathComponent("manifest.json").path)
            return
        }
        if fileManager.fileExists(atPath: installedExtensionManifest) {
            reveal(installedExtensionManifest)
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL])
    }

    private static func syncBundledExtensionToApplicationSupport() {
        guard let bundledDir = bundledExtensionDirectory else { return }
        let fileManager = FileManager.default
        let supportURL = URL(fileURLWithPath: applicationSupportDirectory, isDirectory: true)
        let targetExtURL = URL(fileURLWithPath: installedExtensionDirectory, isDirectory: true)

        do {
            try fileManager.createDirectory(at: supportURL, withIntermediateDirectories: true)
            if fileManager.fileExists(atPath: targetExtURL.path) {
                try fileManager.removeItem(at: targetExtURL)
            }
            try fileManager.copyItem(at: bundledDir, to: targetExtURL)

            if let bundledXpi = bundledExtensionXpi,
               fileManager.fileExists(atPath: bundledXpi.path) {
                let targetXpiURL = URL(fileURLWithPath: installedExtensionXpi)
                if fileManager.fileExists(atPath: targetXpiURL.path) {
                    try fileManager.removeItem(at: targetXpiURL)
                }
                try fileManager.copyItem(at: bundledXpi, to: targetXpiURL)
            }
        } catch {
            NSLog("[%@] could not stage extension into %@: %@", AppInfo.name, applicationSupportDirectory, String(describing: error))
        }
    }

    /// The repository README. The app is normally run straight out of
    /// `<checkout>/build/`; when installed elsewhere, fall back to the bundled
    /// README or revealing the bundle instead of failing.
    static func openReadme() {
        for candidate in readmeCandidates where FileManager.default.fileExists(atPath: candidate) {
            NSWorkspace.shared.open(URL(fileURLWithPath: candidate))
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([Bundle.main.bundleURL])
    }

    /// Walks up from the bundle, looking for the checkout's README.md, then falls
    /// back to the copy bundled inside `Contents/Resources/`.
    static var readmeCandidates: [String] {
        var directory = Bundle.main.bundleURL.deletingLastPathComponent()
        var candidates: [String] = []
        for _ in 0..<4 {
            candidates.append(directory.appendingPathComponent("README.md").path)
            directory.deleteLastPathComponent()
        }
        if let bundled = Bundle.main.resourceURL?.appendingPathComponent("README.md").path {
            candidates.append(bundled)
        }
        return candidates
    }

    static func reveal(_ path: String) {
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
}
