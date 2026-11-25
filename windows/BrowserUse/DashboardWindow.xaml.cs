using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;

namespace BrowserUse;

public partial class DashboardWindow : Window
{
    private readonly BrowserUseModel _model;
    private readonly Action _openSettings;
    private readonly ICollectionView _tabsView;
    private bool _allowClose;

    internal DashboardWindow(BrowserUseModel model, Action openSettings)
    {
        InitializeComponent();
        _model = model;
        _openSettings = openSettings;
        DataContext = model;

        _tabsView = CollectionViewSource.GetDefaultView(model.Tabs);
        _tabsView.Filter = FilterTab;
        TabsListBox.ItemsSource = _tabsView;

        _model.PropertyChanged += Model_PropertyChanged;
        UpdateEmptyState();
    }

    public void ForceClose()
    {
        _allowClose = true;
        Close();
    }

    protected override void OnClosing(CancelEventArgs e)
    {
        if (!_allowClose)
        {
            e.Cancel = true;
            Hide();
            return;
        }
        _model.PropertyChanged -= Model_PropertyChanged;
        base.OnClosing(e);
    }

    private void Model_PropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(BrowserUseModel.ExtensionConnected)
            or nameof(BrowserUseModel.TabCount)
            or nameof(BrowserUseModel.Phase))
        {
            _tabsView.Refresh();
            UpdateEmptyState();
        }
    }

    private bool FilterTab(object obj)
    {
        if (obj is not BrowserUseTab tab) return false;
        string query = (SearchBox.Text ?? string.Empty).Trim();
        if (string.IsNullOrEmpty(query)) return true;

        return tab.DisplayTitle.Contains(query, StringComparison.OrdinalIgnoreCase)
            || (tab.Url?.Contains(query, StringComparison.OrdinalIgnoreCase) ?? false);
    }

    private void UpdateEmptyState()
    {
        bool hasVisibleTabs = !_tabsView.IsEmpty;
        EmptyStatePanel.Visibility = hasVisibleTabs ? Visibility.Collapsed : Visibility.Visible;
        if (_model.ExtensionConnected)
        {
            EmptyStateTitle.Text = "No matching tabs";
            EmptyStateSubtitle.Text = "Open a tab in Zen, or clear the search.";
        }
        else
        {
            EmptyStateTitle.Text = "No browser connection";
            EmptyStateSubtitle.Text = "Connect the Gecko Orange extension to list tabs here.";
        }
    }

    private void SearchBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        _tabsView.Refresh();
        UpdateEmptyState();
    }

    private void Refresh_Click(object sender, RoutedEventArgs e) => _ = _model.RefreshAsync();
    private void RestartHub_Click(object sender, RoutedEventArgs e) => _ = _model.RestartHubAsync();
    private void Settings_Click(object sender, RoutedEventArgs e) => _openSettings();
    private void CopySecret_Click(object sender, RoutedEventArgs e) => _ = _model.CopySecretAsync();
    private void CopySetupCommand_Click(object sender, RoutedEventArgs e) => _ = _model.CopyZenSetupCommandAsync();
    private void OpenExtension_Click(object sender, RoutedEventArgs e) => BrowserUsePaths.OpenExtensionFolder();
    private void RevealLogs_Click(object sender, RoutedEventArgs e) => BrowserUsePaths.OpenLogs();

    private void More_Click(object sender, RoutedEventArgs e)
    {
        if (sender is System.Windows.Controls.Button btn && btn.ContextMenu is { } menu)
        {
            menu.PlacementTarget = btn;
            menu.IsOpen = true;
        }
    }

    private void SwitchTabButton_Click(object sender, RoutedEventArgs e)
    {
        if (sender is System.Windows.Controls.Button { Tag: BrowserUseTab tab })
        {
            _ = _model.ActivateTabAsync(tab);
        }
    }

    private void SwitchTabMenu_Click(object sender, RoutedEventArgs e)
    {
        if (sender is MenuItem { Tag: BrowserUseTab tab })
        {
            _ = _model.ActivateTabAsync(tab);
        }
    }

    private void CopyTabUrl_Click(object sender, RoutedEventArgs e)
    {
        if (sender is MenuItem { Tag: BrowserUseTab tab })
        {
            _model.CopyToClipboard(tab.Url ?? string.Empty);
        }
    }
}
