import AppKit
import SwiftUI

struct DashboardWindow: View {

    @Environment(BrowserUseModel.self) private var model
    @State private var searchText = ""

    var body: some View {
        VStack(spacing: 0) {
            statusCard
            Divider()
            tabsSection
        }
        .searchable(text: $searchText, placement: .toolbar, prompt: "Search tabs")
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await model.refresh() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .disabled(model.isRefreshing || !model.isHubRunning)
            }

            ToolbarItem {
                Button {
                    Task { await model.restartHub() }
                } label: {
                    Label("Restart Hub", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(model.isHubBusy)
            }

            ToolbarItem(placement: .primaryAction) {
                Menu {
                    Button("Settings…") { WindowOpener.shared.openSettings() }
                    Divider()
                    Button("Copy Shared Secret") {
                        Task { await model.copySecret() }
                    }
                    Button("Copy Zen Setup Command") {
                        Task { await model.copyZenSetupCommand() }
                    }
                    Button("Reveal Gecko Orange Extension") {
                        BrowserUsePaths.openExtensionFolder()
                    }
                    Divider()
                    Button("Reveal Hub Logs") { BrowserUsePaths.openLogs() }
                } label: {
                    Label("More", systemImage: "ellipsis.circle")
                }
            }
        }
        .frame(minWidth: 720, minHeight: 480)
        .task { await model.refresh() }
    }

    // MARK: - Status

    private var statusCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 16) {
                ZStack {
                    RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .fill(model.phase.tint.opacity(0.12))
                        .frame(width: 58, height: 58)
                    Image(systemName: model.phase.symbol)
                        .font(.system(size: 27, weight: .semibold))
                        .foregroundStyle(model.phase.tint)
                }

                VStack(alignment: .leading, spacing: 4) {
                    Text(model.phase.title)
                        .font(.title2.weight(.semibold))
                    Text(model.hubURL)
                        .font(.system(.callout, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                    HStack(spacing: 8) {
                        Text(model.extensionConnected ? "Gecko Orange \(model.extensionVersion)" : "Extension not connected")
                        if let updated = model.lastUpdated {
                            Text("·")
                            Text("Updated \(updated.formatted(date: .omitted, time: .standard))")
                        }
                    }
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                }

                Spacer(minLength: 16)

                HStack(spacing: 22) {
                    MetricCell(label: "Tabs", value: "\(model.status?.tabCount ?? 0)")
                    MetricCell(label: "Peers", value: "\(model.status?.peers ?? 0)")
                    MetricCell(label: "Uptime", value: model.uptimeText)
                }
            }

            if let notice = model.notice, !notice.isEmpty {
                Label(notice, systemImage: "info.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }

            if let error = model.lastError, !error.isEmpty,
               model.phase == .unauthorized || model.phase == .error {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .lineLimit(3)
                    .textSelection(.enabled)
            }

            if !model.extensionConnected {
                setupCard
            }

            HStack(spacing: 8) {
                Button("Restart Hub") {
                    Task { await model.restartHub() }
                }
                .disabled(model.isHubBusy)

                Button("Copy Zen Setup Command") {
                    Task { await model.copyZenSetupCommand() }
                }
                Button("Settings…") { WindowOpener.shared.openSettings() }
                Spacer()
                if model.isHubBusy || model.isRefreshing {
                    ProgressView().controlSize(.small)
                }
            }
        }
        .padding(20)
    }

    private var setupCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Finish connecting Zen", systemImage: "wand.and.stars")
                .font(.headline)

            Text("1. In Zen open **about:debugging → This Zen → Load Temporary Add-on…** and select **extension/manifest.json**.\n2. Click the **Gecko Orange** toolbar icon (or **Inspect → Console**), paste your setup command or secret, and connect.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 8) {
                Button("Copy Setup Command") {
                    Task { await model.copyZenSetupCommand() }
                }
                .buttonStyle(.borderedProminent)
                Button("Open Extension Folder") {
                    BrowserUsePaths.openExtensionFolder()
                }
                Button("Copy Shared Secret") {
                    Task { await model.copySecret() }
                }
                Button("Reveal Hub Logs") { BrowserUsePaths.openLogs() }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(Color.orange.opacity(0.08))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(Color.orange.opacity(0.18))
        )
    }

    // MARK: - Tabs

    private var filteredTabs: [BrowserUseTab] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return model.tabs }
        return model.tabs.filter {
            $0.displayTitle.localizedCaseInsensitiveContains(query)
                || ($0.url ?? "").localizedCaseInsensitiveContains(query)
        }
    }

    private var tabsSection: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Open Tabs")
                    .font(.headline)
                Text("\(model.tabs.count)")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(Color.secondary.opacity(0.12)))
                Spacer()
                Button {
                    Task { await model.refresh() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                        .labelStyle(.iconOnly)
                }
                .buttonStyle(.borderless)
                .help("Refresh tabs")
                .disabled(model.isRefreshing || !model.isHubRunning)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)

            if filteredTabs.isEmpty {
                ContentUnavailableView {
                    Label(
                        model.extensionConnected ? "No matching tabs" : "No browser connection",
                        systemImage: model.extensionConnected ? "rectangle.on.rectangle.slash" : "globe"
                    )
                } description: {
                    Text(model.extensionConnected
                         ? "Open a tab in Zen, or clear the search."
                         : "Connect the Gecko Orange extension to list tabs here.")
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(filteredTabs) { tab in
                    BrowserUseTabRow(tab: tab) {
                        Task { await model.activateTab(tab) }
                    }
                }
                .listStyle(.inset)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct MetricCell: View {
    let label: String
    let value: String

    var body: some View {
        VStack(alignment: .trailing, spacing: 2) {
            Text(value)
                .font(.system(size: 19, weight: .semibold, design: .rounded))
                .monospacedDigit()
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

private struct BrowserUseTabRow: View {
    let tab: BrowserUseTab
    let switchToTab: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Circle()
                .fill(tab.isActive ? Color.accentColor : Color.secondary.opacity(0.25))
                .frame(width: 8, height: 8)

            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(tab.displayTitle)
                        .fontWeight(tab.isActive ? .semibold : .regular)
                        .lineLimit(1)
                    if tab.isActive {
                        Text("ACTIVE")
                            .font(.system(size: 9, weight: .semibold))
                            .foregroundStyle(Color.accentColor)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .background(Capsule().fill(Color.accentColor.opacity(0.12)))
                    }
                    if tab.pinned == true {
                        Image(systemName: "pin.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(.secondary)
                    }
                }
                Text(tab.url ?? "")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            Spacer(minLength: 12)

            if !tab.isActive {
                Button("Switch", action: switchToTab)
                    .buttonStyle(.borderless)
            }
        }
        .padding(.vertical, 4)
        .contextMenu {
            Button("Switch to Tab", action: switchToTab)
            Divider()
            Button("Copy URL") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(tab.url ?? "", forType: .string)
            }
        }
    }
}
