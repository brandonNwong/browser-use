import AppKit
import SwiftUI

@MainActor
final class BrowserUseStatusMenu: StatusMenuProviding {

    private let model: BrowserUseModel

    init(model: BrowserUseModel) {
        self.model = model
    }

    func makeHeaderView() -> AnyView {
        AnyView(BrowserUseHeaderView(model: model))
    }

    func makeDomainMenuItems() -> [NSMenuItem] {
        // Rebuild on open, so refreshing from the menu is always current.
        Task { [model] in
            await model.refresh()
        }

        var items: [NSMenuItem] = []

        items.append(infoItem(
            title: extensionLine,
            symbol: model.extensionConnected ? "checkmark.circle" : "circle.dotted"
        ))
        items.append(infoItem(title: hubLine, symbol: hubSymbol))

        if let error = model.lastError, model.phase == .unauthorized || model.phase == .error {
            items.append(infoItem(title: truncated(error, length: 72), symbol: "exclamationmark.triangle"))
        }

        if BrowserUseDefaultsKey.showTabsInMenu.value {
            items.append(.separator())
            items.append(tabsItem())
        }

        items.append(.separator())
        items.append(copySecretItem())
        items.append(copySetupCommandItem())
        items.append(ActionMenuItem(title: "Open Extension Folder") {
            BrowserUsePaths.openExtensionFolder()
        })

        let restart = ActionMenuItem(title: "Restart Hub") { [model] in
            Task { await model.restartHub() }
        }
        restart.isEnabled = !model.isHubBusy
        items.append(restart)

        items.append(ActionMenuItem(title: "Reveal Hub Logs") {
            BrowserUsePaths.openLogs()
        })

        return items
    }

    // MARK: - Rows

    private var extensionLine: String {
        if model.extensionConnected {
            let version = model.status?.extensionInfo?.version
            return "Zen extension: Connected" + (version.map { " · \($0)" } ?? "")
        }
        return "Zen extension: Not connected"
    }

    private var hubLine: String {
        let peers = model.status?.peers ?? 0
        return "Hub: \(model.hubStateWord) at \(model.hubURL) · \(peers) peer\(peers == 1 ? "" : "s")"
    }

    private var hubSymbol: String {
        if model.isHubBusy { return "hourglass" }
        return model.isHubRunning ? "network" : "network.slash"
    }

    private func infoItem(title: String, symbol: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)
        item.image?.isTemplate = true
        item.isEnabled = false
        return item
    }

    private func tabsItem() -> NSMenuItem {
        let item = NSMenuItem(
            title: "Tabs (\(model.tabs.count))",
            action: nil,
            keyEquivalent: ""
        )
        let submenu = NSMenu()
        let tabs = model.tabs

        if tabs.isEmpty {
            let empty = NSMenuItem(
                title: model.extensionConnected ? "No open tabs" : "Extension not connected",
                action: nil,
                keyEquivalent: ""
            )
            empty.isEnabled = false
            submenu.addItem(empty)
        } else {
            for tab in tabs.prefix(model.menuTabLimit) {
                let tabItem = ActionMenuItem(
                    title: truncated(tab.displayTitle, length: 52),
                    state: tab.isActive ? .on : .off
                ) { [model] in
                    Task { await model.activateTab(tab) }
                }
                tabItem.toolTip = tab.url
                submenu.addItem(tabItem)
            }

            if tabs.count > model.menuTabLimit {
                submenu.addItem(.separator())
                submenu.addItem(ActionMenuItem(title: "More Tabs…") {
                    WindowOpener.shared.openMain()
                })
            }
        }

        submenu.addItem(.separator())
        submenu.addItem(ActionMenuItem(title: "Refresh Tabs") { [model] in
            Task { await model.refresh() }
        })
        submenu.addItem(ActionMenuItem(title: "Open Dashboard…") {
            WindowOpener.shared.openMain()
        })

        item.submenu = submenu
        return item
    }

    private func copySecretItem() -> NSMenuItem {
        ActionMenuItem(title: "Copy Shared Secret") { [model] in
            Task { await model.copySecret() }
        }
    }

    private func copySetupCommandItem() -> NSMenuItem {
        ActionMenuItem(title: "Copy Zen Setup Command") { [model] in
            Task { await model.copyZenSetupCommand() }
        }
    }

    private func truncated(_ text: String, length: Int) -> String {
        guard text.count > length else { return text }
        return String(text.prefix(max(1, length - 1))) + "…"
    }
}

struct BrowserUseHeaderView: View {

    let model: BrowserUseModel

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            ZStack {
                Circle()
                    .fill(model.phase.tint.opacity(0.14))
                    .frame(width: 30, height: 30)
                Image(systemName: model.phase.symbol)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(model.phase.tint)
            }

            VStack(alignment: .leading, spacing: 1) {
                Text(AppInfo.name)
                    .font(.system(size: Metrics.Font.title, weight: .semibold))
                Text(model.headline)
                    .font(.system(size: Metrics.Font.detail))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            Text(statusPill)
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(model.phase.tint)
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(
                    Capsule().fill(model.phase.tint.opacity(0.12))
                )
        }
        .padding(.horizontal, Metrics.Menu.itemInset)
        .padding(.vertical, Metrics.Menu.headerVerticalInset)
        .frame(width: Metrics.Menu.width, alignment: .leading)
    }

    private var statusPill: String {
        if model.extensionConnected { return "LIVE" }
        switch model.phase {
        case .hubOnline: return "LIVE"
        case .waitingForExtension: return "WAITING"
        case .starting: return "STARTING"
        case .unauthorized: return "SECRET"
        case .offline: return "OFFLINE"
        case .error: return "ERROR"
        }
    }
}
