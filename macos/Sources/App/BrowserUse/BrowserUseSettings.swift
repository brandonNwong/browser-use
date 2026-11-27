import AppKit
import SwiftUI

struct SettingsWindow: View {

    var body: some View {
        TabView {
            GeneralSettings()
                .tabItem { Label("General", systemImage: "slider.horizontal.3") }

            ConnectionSettings()
                .tabItem { Label("Connection", systemImage: "globe") }

            AboutSettings()
                .tabItem { Label("About", systemImage: "info.circle") }
        }
        .frame(
            width: Metrics.Preferences.windowWidth,
            height: Metrics.Preferences.windowHeight
        )
    }
}

// MARK: - General

struct GeneralSettings: View {

    @Environment(BrowserUseModel.self) private var model
    @AppStorage(BrowserUseDefaultsKey.refreshSeconds) private var refreshSeconds
    @AppStorage(BrowserUseDefaultsKey.showTabsInMenu) private var showTabsInMenu
    @AppStorage(BrowserUseDefaultsKey.menuTabLimit) private var menuTabLimit
    @State private var portText = ""

    var body: some View {
        PreferencePane {
            PreferenceRow(label: "In-process hub:") {
                HStack(spacing: 8) {
                    StatusBadge(
                        text: model.hubSummary,
                        color: model.isHubRunning ? .green : .secondary
                    )
                    Spacer()
                    if model.isHubBusy { ProgressView().controlSize(.small) }
                }

                HStack(spacing: 8) {
                    Button("Restart") {
                        Task { await model.restartHub() }
                    }
                    .disabled(model.isHubBusy)

                    Text(model.hubURL)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)

                    Spacer()
                }

                Text("The hub runs inside \(AppInfo.name) on the loopback interface. Opening the app starts it; quitting the app stops it. MCP clients and the CLI connect to it over WebSocket.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                if let notice = model.notice, !notice.isEmpty {
                    Text(notice)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }

            PreferenceRow(label: "Hub port:") {
                HStack(spacing: 8) {
                    TextField("19455", text: $portText)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)
                        .onSubmit { applyPort() }

                    Button("Apply & Restart") { applyPort() }
                        .disabled(parsedPort == nil || parsedPort == model.port || model.isHubBusy)

                    Spacer()
                }

                Text("MCP clients use this port too. The default is 19455. Changing it restarts the in-process hub; Zen's orange_bridge.url must match it.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            PreferenceRow(label: "Menu bar:") {
                Toggle("Show open tabs in the menu", isOn: $showTabsInMenu)
                    .toggleStyle(.checkbox)

                Stepper("Menu tab limit: \(menuTabLimit)", value: $menuTabLimit, in: 3...20)
                    .disabled(!showTabsInMenu)
            }

            PreferenceRow(label: "Refresh:") {
                Picker("", selection: $refreshSeconds) {
                    Text("1 second").tag(1)
                    Text("2 seconds").tag(2)
                    Text("5 seconds").tag(5)
                    Text("10 seconds").tag(10)
                }
                .labelsHidden()
                .fixedSize()
                .onChange(of: refreshSeconds) { _, _ in
                    model.startPolling()
                }
            }

            PreferenceRow(label: "Window:") {
                HideDockIconRow()
            }

            PreferenceRow(label: "Login:") {
                LaunchAtLoginToggle()
                Text("Starting \(AppInfo.name) at login also starts the in-process hub.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            PreferenceRow(label: "Reset:") {
                ResetPreferenceRow()
            }
        }
        .onAppear { portText = String(model.port) }
    }

    private var parsedPort: Int? {
        guard let value = Int(portText.trimmingCharacters(in: .whitespaces)), (1...65_535).contains(value) else {
            return nil
        }
        return value
    }

    private func applyPort() {
        guard let port = parsedPort else { return }
        Task { await model.restartHub(port: port) }
    }
}

// MARK: - Connection

struct ConnectionSettings: View {

    @Environment(BrowserUseModel.self) private var model
    @State private var isRevealed = false

    var body: some View {
        PreferencePane {
            PreferenceRow(label: "Hub:") {
                SettingsValueRow(label: "Address", value: model.hubURL, monospaced: true)
                SettingsValueRow(label: "State", value: model.hubSummary)
                SettingsValueRow(label: "Extension", value: model.extensionConnected ? "Connected" : "Not connected")
                SettingsValueRow(label: "Version", value: model.extensionVersion)
                SettingsValueRow(label: "Protocol", value: model.status?.protocolVersion ?? "—")
                SettingsValueRow(label: "Peers", value: "\(model.status?.peers ?? 0)")
                SettingsValueRow(label: "Uptime", value: model.uptimeText)
            }

            PreferenceRow(label: "Shared secret:") {
                HStack(spacing: 8) {
                    Text(secretDisplay)
                        .font(.system(.body, design: .monospaced))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 5)
                        .background(
                            RoundedRectangle(cornerRadius: 6)
                                .fill(Color(nsColor: .textBackgroundColor))
                        )

                    Button(isRevealed ? "Hide" : "Reveal") {
                        isRevealed.toggle()
                    }
                    .disabled(model.secretInfo == nil)
                }

                Text("Stored at \(BrowserUsePaths.displaySecretFile) with owner-only permissions.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)

                HStack(spacing: 8) {
                    Button("Copy Secret") {
                        Task { await model.copySecret() }
                    }
                    Button("Copy Zen Setup Command") {
                        Task { await model.copyZenSetupCommand() }
                    }
                    Spacer()
                }

                HStack(spacing: 8) {
                    Button("Open Extension Folder") {
                        BrowserUsePaths.openExtensionFolder()
                    }
                    Spacer()
                }

                Text("Load extension/manifest.json in Zen (about:debugging → This Zen → Load Temporary Add-on…), then click the Gecko Orange toolbar icon (or Inspect → Console) and paste the secret or setup command.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            PreferenceRow(label: "Connection:") {
                HStack(spacing: 8) {
                    Button("Restart Hub") {
                        Task { await model.restartHub() }
                    }
                    .disabled(model.isHubBusy)

                    Button("Refresh Now") {
                        Task { await model.refresh() }
                    }
                    .disabled(model.isRefreshing || !model.isHubRunning)

                    Spacer()
                }

                if let error = model.lastError, !error.isEmpty {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
        }
        .task {
            // Rendering (preview data) already provides a secret; only a real
            // session asks the FFI, which may create ~/.orange-mcp/secret.
            if model.secretInfo == nil {
                await model.loadSecretInfo()
            }
        }
    }

    private var secretDisplay: String {
        if isRevealed, let secret = model.revealSecret(), !secret.isEmpty {
            return secret
        }
        return "••••••••••••••••••••••••"
    }
}

// MARK: - About

struct AboutSettings: View {

    @Environment(BrowserUseModel.self) private var model

    var body: some View {
        PreferencePane {
            PreferenceRow(label: "\(AppInfo.name):") {
                HStack(spacing: 12) {
                    Image(systemName: "globe")
                        .font(.system(size: 34))
                        .foregroundStyle(.tint)
                        .frame(width: 40)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(AppInfo.name).fontWeight(.semibold)
                        Text("Version \(AppInfo.version)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                }
            }

            PreferenceRow(label: "Hub:") {
                SettingsValueRow(label: "Address", value: model.hubURL, monospaced: true)
                SettingsValueRow(label: "State", value: model.hubSummary)
                SettingsValueRow(label: "Uptime", value: model.uptimeText)
            }

            PreferenceRow(label: "Resources:") {
                HStack(spacing: 12) {
                    Button("README") { BrowserUsePaths.openReadme() }
                    Button("Reveal Hub Logs") { BrowserUsePaths.openLogs() }
                    Button("Secret Location") { BrowserUsePaths.reveal(BrowserUsePaths.secretFile) }
                    Spacer()
                }
            }

            PreferenceRow(label: "Behavior:") {
                Text("The hub runs inside this app, so MCP clients and terminal commands share one browser connection while \(AppInfo.name) is open. Quitting the app stops the hub.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: - Small shared controls

struct SettingsValueRow: View {

    let label: String
    let value: String
    var monospaced = false

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .foregroundStyle(.secondary)
            Spacer(minLength: 12)
            Text(value)
                .font(monospaced ? .system(.caption, design: .monospaced) : .caption)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .lineLimit(2)
                .truncationMode(.middle)
        }
    }
}

struct StatusBadge: View {

    let text: String
    let color: Color

    var body: some View {
        HStack(spacing: 5) {
            Circle()
                .fill(color)
                .frame(width: 7, height: 7)
            Text(text)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}
