using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;

namespace BrowserUse;

public partial class SettingsWindow : Window
{
    private readonly BrowserUseModel _model;
    private bool _allowClose;
    private bool _isRevealed;
    private bool _suppressSelectionEvents;

    internal SettingsWindow(BrowserUseModel model)
    {
        InitializeComponent();
        _model = model;
        DataContext = model;

        for (int i = 3; i <= 20; i++)
        {
            TabLimitCombo.Items.Add(i);
        }

        SecretPathHint.Text = $"Stored at {BrowserUsePaths.DisplaySecretFile}.";
        SyncControlsFromModel();

        _model.PropertyChanged += Model_PropertyChanged;
        Loaded += async (_, _) =>
        {
            if (!_model.HasSecret)
            {
                await _model.LoadSecretInfoAsync().ConfigureAwait(true);
                UpdateSecretDisplay();
            }
        };
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
        if (e.PropertyName is nameof(BrowserUseModel.Port)
            or nameof(BrowserUseModel.RefreshInterval)
            or nameof(BrowserUseModel.MenuTabLimit)
            or nameof(BrowserUseModel.ExtensionConnected)
            or nameof(BrowserUseModel.HasSecret))
        {
            SyncControlsFromModel();
        }
    }

    private void SyncControlsFromModel()
    {
        _suppressSelectionEvents = true;
        try
        {
            PortTextBox.Text = _model.Port.ToString();
            TabLimitCombo.SelectedItem = _model.MenuTabLimit;
            foreach (ComboBoxItem item in RefreshCombo.Items)
            {
                if (item.Tag is string tag && int.TryParse(tag, out int val) && val == _model.RefreshInterval)
                {
                    RefreshCombo.SelectedItem = item;
                    break;
                }
            }
            LaunchAtLoginCheckBox.IsChecked = StartupManager.IsEnabled;
            ExtensionStatusValue.Text = _model.ExtensionConnected ? "Connected" : "Not connected";
            UpdateSecretDisplay();
        }
        finally
        {
            _suppressSelectionEvents = false;
        }
    }

    private void UpdateSecretDisplay()
    {
        if (_isRevealed && _model.SecretInfo is { } info && !string.IsNullOrEmpty(info.Secret))
        {
            SecretTextBox.Text = info.Secret;
        }
        else
        {
            SecretTextBox.Text = "••••••••••••••••••••••••";
        }
        RevealSecretButton.Content = _isRevealed ? "Hide" : "Reveal";
    }

    private void ApplyPort_Click(object sender, RoutedEventArgs e)
    {
        if (int.TryParse(PortTextBox.Text.Trim(), out int port) && port is >= 1 and <= 65_535)
        {
            _ = _model.RestartHubAsync(port);
        }
        else
        {
            System.Windows.MessageBox.Show(
                this,
                "Port must be an integer between 1 and 65535.",
                "Invalid Port",
                MessageBoxButton.OK,
                MessageBoxImage.Warning);
        }
    }

    private void TabLimitCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_suppressSelectionEvents) return;
        if (TabLimitCombo.SelectedItem is int limit)
        {
            _model.SetMenuTabLimit(limit);
        }
    }

    private void RefreshCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_suppressSelectionEvents) return;
        if (RefreshCombo.SelectedItem is ComboBoxItem { Tag: string tag } && int.TryParse(tag, out int seconds))
        {
            _model.SetRefreshInterval(seconds);
        }
    }

    private void LaunchAtLogin_Click(object sender, RoutedEventArgs e)
    {
        bool desired = LaunchAtLoginCheckBox.IsChecked == true;
        try
        {
            StartupManager.SetEnabled(desired);
            LaunchAtLoginErrorText.Visibility = Visibility.Collapsed;
        }
        catch (Exception ex)
        {
            LaunchAtLoginErrorText.Text = ex.Message;
            LaunchAtLoginErrorText.Visibility = Visibility.Visible;
        }
        LaunchAtLoginCheckBox.IsChecked = StartupManager.IsEnabled;
    }

    private void RevealSecret_Click(object sender, RoutedEventArgs e)
    {
        _isRevealed = !_isRevealed;
        UpdateSecretDisplay();
    }

    private void RestartHub_Click(object sender, RoutedEventArgs e) => _ = _model.RestartHubAsync();
    private void Refresh_Click(object sender, RoutedEventArgs e) => _ = _model.RefreshAsync();
    private void CopySecret_Click(object sender, RoutedEventArgs e) => _ = _model.CopySecretAsync();
    private void CopySetupCommand_Click(object sender, RoutedEventArgs e) => _ = _model.CopyZenSetupCommandAsync();
    private void OpenExtension_Click(object sender, RoutedEventArgs e) => BrowserUsePaths.OpenExtensionFolder();
    private void Reset_Click(object sender, RoutedEventArgs e) => _model.ResetPreferences();
    private void OpenReadme_Click(object sender, RoutedEventArgs e) => BrowserUsePaths.OpenReadme();
    private void RevealLogs_Click(object sender, RoutedEventArgs e) => BrowserUsePaths.OpenLogs();
    private void RevealSecretLocation_Click(object sender, RoutedEventArgs e) => BrowserUsePaths.Reveal(BrowserUsePaths.SecretFile);
}
