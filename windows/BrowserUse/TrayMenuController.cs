using System.Drawing;
using System.IO;
using System.Windows;
using WinForms = System.Windows.Forms;

namespace BrowserUse;

/// <summary>
/// Owns the Windows notification area (system tray) icon and context menu.
/// Mirrors <c>macos/Sources/App/Shell/StatusItemController.swift</c> +
/// <c>macos/Sources/App/BrowserUse/BrowserUseStatusMenu.swift</c>.
/// </summary>
internal sealed class TrayMenuController : IDisposable
{
    private readonly BrowserUseModel _model;
    private readonly Action _openDashboard;
    private readonly Action _openSettings;
    private readonly Action _quitApp;
    private readonly WinForms.NotifyIcon _notifyIcon;
    private readonly WinForms.ContextMenuStrip _menu;
    private Icon? _trayIcon;

    public TrayMenuController(
        BrowserUseModel model,
        Action openDashboard,
        Action openSettings,
        Action quitApp,
        bool createTrayIcon = true)
    {
        _model = model;
        _openDashboard = openDashboard;
        _openSettings = openSettings;
        _quitApp = quitApp;

        _menu = new WinForms.ContextMenuStrip();
        _menu.Opening += (_, _) => RebuildMenu(_menu);

        _notifyIcon = new WinForms.NotifyIcon();
        if (createTrayIcon)
        {
            _trayIcon = LoadAppIcon();
            _notifyIcon.Icon = _trayIcon ?? SystemIcons.Application;
            _notifyIcon.Text = "Browser Use";
            _notifyIcon.ContextMenuStrip = _menu;
            _notifyIcon.Visible = true;
            _notifyIcon.DoubleClick += (_, _) => _openDashboard();
        }
    }

    public IReadOnlyList<string> DumpMenuLines()
    {
        using var previewMenu = new WinForms.ContextMenuStrip();
        RebuildMenu(previewMenu);

        var lines = new List<string>();
        foreach (WinForms.ToolStripItem item in previewMenu.Items)
        {
            DumpItem(item, 0, lines);
        }
        return lines;
    }

    private static void DumpItem(WinForms.ToolStripItem item, int depth, List<string> lines)
    {
        string indent = new(' ', depth * 2);
        if (item is WinForms.ToolStripSeparator)
        {
            lines.Add($"{indent}---");
            return;
        }

        string text = item.Text ?? string.Empty;
        if (item is WinForms.ToolStripMenuItem menuItem)
        {
            string check = menuItem.Checked ? "[x] " : string.Empty;
            lines.Add($"{indent}{check}{text}");
            if (menuItem.HasDropDownItems)
            {
                foreach (WinForms.ToolStripItem child in menuItem.DropDownItems)
                {
                    DumpItem(child, depth + 1, lines);
                }
            }
        }
        else
        {
            lines.Add($"{indent}{text}");
        }
    }

    private void RebuildMenu(WinForms.ContextMenuStrip menu)
    {
        menu.Items.Clear();

        _ = _model.RefreshAsync();

        var header = new WinForms.ToolStripMenuItem($"Browser Use — {_model.Headline} [{_model.StatusPill}]")
        {
            Enabled = false,
            Font = new Font(menu.Font, System.Drawing.FontStyle.Bold)
        };
        menu.Items.Add(header);

        string extensionText = _model.ExtensionConnected
            ? $"Zen extension: Connected{(_model.Status?.ExtensionInfo?.Version is { } v ? $" · {v}" : string.Empty)}"
            : "Zen extension: Not connected";
        menu.Items.Add(new WinForms.ToolStripMenuItem(extensionText) { Enabled = false });

        int peers = _model.Peers;
        string hubText = $"Hub: {_model.HubStateWord} at {_model.HubUrl} · {peers} peer{(peers == 1 ? string.Empty : "s")}";
        menu.Items.Add(new WinForms.ToolStripMenuItem(hubText) { Enabled = false });

        if (!string.IsNullOrWhiteSpace(_model.LastError)
            && (_model.Phase == HubPhase.Unauthorized || _model.Phase == HubPhase.Error))
        {
            menu.Items.Add(new WinForms.ToolStripMenuItem(Truncate(_model.LastError, 72)) { Enabled = false });
        }

        if (_model.ShowTabsInMenu)
        {
            menu.Items.Add(new WinForms.ToolStripSeparator());
            menu.Items.Add(BuildTabsItem());
        }

        menu.Items.Add(new WinForms.ToolStripSeparator());
        menu.Items.Add(new WinForms.ToolStripMenuItem("Copy Shared Secret", null, (_, _) => _ = _model.CopySecretAsync()));
        menu.Items.Add(new WinForms.ToolStripMenuItem("Copy Zen Setup Command", null, (_, _) => _ = _model.CopyZenSetupCommandAsync()));
        menu.Items.Add(new WinForms.ToolStripMenuItem("Open Extension Folder", null, (_, _) => BrowserUsePaths.OpenExtensionFolder()));

        var restartItem = new WinForms.ToolStripMenuItem("Restart Hub", null, (_, _) => _ = _model.RestartHubAsync())
        {
            Enabled = !_model.IsHubBusy
        };
        menu.Items.Add(restartItem);
        menu.Items.Add(new WinForms.ToolStripMenuItem("Reveal Hub Logs", null, (_, _) => BrowserUsePaths.OpenLogs()));

        menu.Items.Add(new WinForms.ToolStripSeparator());
        menu.Items.Add(new WinForms.ToolStripMenuItem("Open Dashboard…", null, (_, _) => _openDashboard()));
        menu.Items.Add(new WinForms.ToolStripMenuItem("Settings…", null, (_, _) => _openSettings()));
        menu.Items.Add(new WinForms.ToolStripSeparator());
        menu.Items.Add(new WinForms.ToolStripMenuItem("Quit Browser Use", null, (_, _) => _quitApp()));
    }

    private WinForms.ToolStripMenuItem BuildTabsItem()
    {
        var tabs = _model.Tabs;
        var root = new WinForms.ToolStripMenuItem($"Tabs ({tabs.Count})");

        if (tabs.Count == 0)
        {
            string emptyText = _model.ExtensionConnected ? "No open tabs" : "Extension not connected";
            root.DropDownItems.Add(new WinForms.ToolStripMenuItem(emptyText) { Enabled = false });
        }
        else
        {
            foreach (BrowserUseTab tab in tabs.Take(_model.MenuTabLimit))
            {
                BrowserUseTab captured = tab;
                var item = new WinForms.ToolStripMenuItem(Truncate(captured.DisplayTitle, 52))
                {
                    Checked = captured.IsActive,
                    ToolTipText = captured.Url ?? string.Empty
                };
                item.Click += (_, _) => _ = _model.ActivateTabAsync(captured);
                root.DropDownItems.Add(item);
            }

            if (tabs.Count > _model.MenuTabLimit)
            {
                root.DropDownItems.Add(new WinForms.ToolStripSeparator());
                root.DropDownItems.Add(new WinForms.ToolStripMenuItem("More Tabs…", null, (_, _) => _openDashboard()));
            }
        }

        root.DropDownItems.Add(new WinForms.ToolStripSeparator());
        root.DropDownItems.Add(new WinForms.ToolStripMenuItem("Refresh Tabs", null, (_, _) => _ = _model.RefreshAsync()));
        root.DropDownItems.Add(new WinForms.ToolStripMenuItem("Open Dashboard…", null, (_, _) => _openDashboard()));
        return root;
    }

    private static string Truncate(string text, int maxLength)
    {
        if (text.Length <= maxLength) return text;
        return text[..Math.Max(1, maxLength - 1)] + "…";
    }

    private static Icon? LoadAppIcon()
    {
        try
        {
            var streamInfo = System.Windows.Application.GetResourceStream(
                new Uri("pack://application:,,,/Resources/AppIcon.ico"));
            if (streamInfo?.Stream is { } stream)
            {
                using (stream)
                {
                    return new Icon(stream, new System.Drawing.Size(32, 32));
                }
            }
        }
        catch
        {
            // Fallback below.
        }

        string? exe = Environment.ProcessPath;
        if (!string.IsNullOrEmpty(exe) && File.Exists(exe))
        {
            try
            {
                return Icon.ExtractAssociatedIcon(exe);
            }
            catch
            {
                // Ignore extraction failure.
            }
        }
        return null;
    }

    public void Dispose()
    {
        _notifyIcon.Visible = false;
        _notifyIcon.Dispose();
        _menu.Dispose();
        _trayIcon?.Dispose();
        _trayIcon = null;
    }
}
