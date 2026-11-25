using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Text.Json;
using System.Windows;
using MediaColor = System.Windows.Media.Color;
using SolidColorBrush = System.Windows.Media.SolidColorBrush;

namespace BrowserUse;

internal enum HubPhase
{
    Starting,
    HubOnline,
    WaitingForExtension,
    Unauthorized,
    Offline,
    Error
}

/// <summary>
/// Domain view-model for Browser Use on Windows.
/// Mirrors <c>macos/Sources/App/BrowserUse/BrowserUseModel.swift</c>.
/// </summary>
internal sealed class BrowserUseModel : INotifyPropertyChanged, IDisposable
{
    private readonly HubBridge _bridge = new();
    private readonly BrowserUseSettingsStore _settings;
    private readonly System.Windows.Threading.Dispatcher? _dispatcher;

    private CancellationTokenSource? _pollCts;
    private bool _isShuttingDown;
    private bool _isRefreshing;
    private bool _isHubBusy;
    private bool _isHubRunning;
    private HubPhase _phase = HubPhase.Starting;
    private BrowserUseStatus? _status;
    private int _port;
    private DateTimeOffset? _lastUpdated;
    private string? _lastError;
    private string? _notice;
    private SecretInfo? _secretInfo;

    public event PropertyChangedEventHandler? PropertyChanged;

    public ObservableCollection<BrowserUseTab> Tabs { get; } = [];

    public BrowserUseModel(BrowserUseSettingsStore settings, bool startsHub = true)
    {
        _settings = settings;
        _port = ResolveInitialPort(settings.Port);
        _dispatcher = System.Windows.Application.Current?.Dispatcher;

        if (startsHub)
        {
            _ = StartHubAsync();
        }
    }

    public static BrowserUseModel CreatePreview()
    {
        var store = new BrowserUseSettingsStore(
            System.IO.Path.Combine(System.IO.Path.GetTempPath(), $"browser-use-preview-{Environment.ProcessId}.json"));
        var model = new BrowserUseModel(store, startsHub: false)
        {
            _phase = HubPhase.HubOnline,
            _isHubRunning = true,
            _port = 19455,
            _lastUpdated = DateTimeOffset.Now,
            _status = new BrowserUseStatus
            {
                Ok = true,
                Service = "browser-use",
                Mode = "hub",
                Port = 19455,
                Pid = Environment.ProcessId,
                ProtocolVersion = "3.0.0",
                UptimeMs = 86_400_000,
                ExtensionConnected = true,
                ExtensionInfo = new BrowserUseStatus.ExtensionInfoModel
                {
                    Version = "3.0.0",
                    Browser = "firefox",
                    ProtocolVersion = "3.0.0",
                    ConnectedAtMs = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
                },
                Peers = 2,
                Tabs =
                [
                    new BrowserUseTab
                    {
                        Id = 42, WindowId = 1, Active = true, Pinned = false, Index = 0,
                        Url = "https://developer.mozilla.org", Title = "MDN Web Docs"
                    },
                    new BrowserUseTab
                    {
                        Id = 43, WindowId = 1, Active = false, Pinned = true, Index = 1,
                        Url = "https://github.com", Title = "GitHub"
                    },
                    new BrowserUseTab
                    {
                        Id = 44, WindowId = 1, Active = false, Pinned = false, Index = 2,
                        Url = "https://example.com/docs/agents", Title = "Agent documentation"
                    }
                ]
            },
            _secretInfo = new SecretInfo
            {
                Secret = "preview-secret-not-real",
                Source = "preview",
                SetupCommand = "await OrangeBridge.setSecret(\"preview-secret-not-real\")"
            }
        };
        model.SyncTabs(model._status.TabsOrEmpty);
        return model;
    }

    public BrowserUseSettingsStore Settings => _settings;

    public HubPhase Phase
    {
        get => _phase;
        private set
        {
            if (_phase == value) return;
            _phase = value;
            NotifyPresentationChanged();
        }
    }

    public BrowserUseStatus? Status
    {
        get => _status;
        private set
        {
            _status = value;
            SyncTabs(value?.TabsOrEmpty ?? []);
            NotifyPresentationChanged();
        }
    }

    public int Port
    {
        get => _port;
        private set
        {
            if (_port == value) return;
            _port = value;
            NotifyPresentationChanged();
        }
    }

    public DateTimeOffset? LastUpdated
    {
        get => _lastUpdated;
        private set
        {
            _lastUpdated = value;
            NotifyPresentationChanged();
        }
    }

    public string? LastError
    {
        get => _lastError;
        private set
        {
            if (_lastError == value) return;
            _lastError = value;
            NotifyPresentationChanged();
        }
    }

    public string? Notice
    {
        get => _notice;
        private set
        {
            if (_notice == value) return;
            _notice = value;
            NotifyPresentationChanged();
        }
    }

    public bool IsRefreshing
    {
        get => _isRefreshing;
        private set
        {
            if (_isRefreshing == value) return;
            _isRefreshing = value;
            NotifyPresentationChanged();
        }
    }

    public bool IsHubBusy
    {
        get => _isHubBusy;
        private set
        {
            if (_isHubBusy == value) return;
            _isHubBusy = value;
            NotifyPresentationChanged();
        }
    }

    public bool IsHubRunning
    {
        get => _isHubRunning;
        private set
        {
            if (_isHubRunning == value) return;
            _isHubRunning = value;
            NotifyPresentationChanged();
        }
    }

    public SecretInfo? SecretInfo
    {
        get => _secretInfo;
        private set
        {
            _secretInfo = value;
            OnPropertyChanged();
            OnPropertyChanged(nameof(HasSecret));
        }
    }

    public bool HasSecret => _secretInfo is not null;
    public bool ExtensionConnected => _status?.ExtensionConnected == true;
    public int TabCount => _status?.TabCount ?? 0;
    public int Peers => _status?.Peers ?? 0;
    public string HubUrl => $"ws://127.0.0.1:{_port}";
    public string ExtensionVersion => _status?.ExtensionInfo?.Version ?? "—";
    public string ProtocolVersion => _status?.ProtocolVersion ?? "—";

    public int RefreshInterval => _settings.RefreshSeconds;
    public int MenuTabLimit => _settings.MenuTabLimit;
    public bool ShowTabsInMenu
    {
        get => _settings.ShowTabsInMenu;
        set
        {
            _settings.ShowTabsInMenu = value;
            OnPropertyChanged();
        }
    }

    public void SetRefreshInterval(int seconds)
    {
        _settings.RefreshSeconds = seconds;
        OnPropertyChanged(nameof(RefreshInterval));
        StartPolling();
    }

    public void SetMenuTabLimit(int limit)
    {
        _settings.MenuTabLimit = limit;
        OnPropertyChanged(nameof(MenuTabLimit));
    }

    public string HubStateWord
    {
        get
        {
            if (_isHubBusy) return "starting";
            return _isHubRunning ? "running" : "offline";
        }
    }

    public string HubSummary
    {
        get
        {
            if (_isHubBusy) return "starting…";
            if (!_isHubRunning) return "offline";
            return ExtensionConnected ? "running · extension connected" : "running · waiting for Zen";
        }
    }

    public string PhaseTitle => _phase switch
    {
        HubPhase.Starting => "Starting hub…",
        HubPhase.HubOnline => "Zen extension connected",
        HubPhase.WaitingForExtension => "Waiting for Zen extension",
        HubPhase.Unauthorized => "Shared secret rejected",
        HubPhase.Offline => "Hub offline",
        HubPhase.Error => "Hub error",
        _ => "Hub error"
    };

    public string StatusPill
    {
        get
        {
            if (ExtensionConnected) return "LIVE";
            return _phase switch
            {
                HubPhase.HubOnline => "LIVE",
                HubPhase.WaitingForExtension => "WAITING",
                HubPhase.Starting => "STARTING",
                HubPhase.Unauthorized => "SECRET",
                HubPhase.Offline => "OFFLINE",
                HubPhase.Error => "ERROR",
                _ => "ERROR"
            };
        }
    }

    public string Headline
    {
        get
        {
            if (ExtensionConnected)
            {
                int count = TabCount;
                return count == 1 ? "Zen connected · 1 tab" : $"Zen connected · {count} tabs";
            }
            return PhaseTitle;
        }
    }

    public string UptimeText
    {
        get
        {
            long? ms = _status?.UptimeMs;
            if (ms is null || ms <= 0) return "—";
            long seconds = ms.Value / 1000;
            if (seconds < 60) return $"{seconds}s";
            long minutes = seconds / 60;
            if (minutes < 60) return $"{minutes}m";
            long hours = minutes / 60;
            if (hours < 24) return $"{hours}h {minutes % 60}m";
            return $"{hours / 24}d {hours % 24}h";
        }
    }

    public string ExtensionSubtitle
    {
        get
        {
            string baseText = ExtensionConnected
                ? $"Gecko Orange {ExtensionVersion}"
                : "Extension not connected";
            if (_lastUpdated is { } updated)
            {
                return $"{baseText} · Updated {updated.LocalDateTime:T}";
            }
            return baseText;
        }
    }

    public bool ShowSetupCard => !ExtensionConnected;
    public bool ShowNoticeBanner => !string.IsNullOrWhiteSpace(_notice);
    public bool ShowErrorBanner => !string.IsNullOrWhiteSpace(_lastError)
        && (_phase == HubPhase.Unauthorized || _phase == HubPhase.Error || _phase == HubPhase.Offline);

    public SolidColorBrush PhaseBrush => _phase switch
    {
        HubPhase.HubOnline => new SolidColorBrush(MediaColor.FromRgb(0x22, 0xC5, 0x5E)),
        HubPhase.Starting or HubPhase.WaitingForExtension => new SolidColorBrush(MediaColor.FromRgb(0xF9, 0x73, 0x16)),
        _ => new SolidColorBrush(MediaColor.FromRgb(0x94, 0xA3, 0xB8))
    };

    public SolidColorBrush PhaseTintBackground => _phase switch
    {
        HubPhase.HubOnline => new SolidColorBrush(MediaColor.FromArgb(0x24, 0x22, 0xC5, 0x5E)),
        HubPhase.Starting or HubPhase.WaitingForExtension => new SolidColorBrush(MediaColor.FromArgb(0x24, 0xF9, 0x73, 0x16)),
        _ => new SolidColorBrush(MediaColor.FromArgb(0x20, 0x94, 0xA3, 0xB8))
    };

    public async Task StartHubAsync()
    {
        if (_isHubBusy || _isShuttingDown) return;
        IsHubBusy = true;
        await LaunchHubAsync(_port, persistPort: false).ConfigureAwait(true);
        IsHubBusy = false;
    }

    public async Task RestartHubAsync(int? newPort = null)
    {
        if (_isHubBusy || _isShuttingDown) return;
        IsHubBusy = true;
        StopPolling();
        await _bridge.StopAsync().ConfigureAwait(true);
        IsHubRunning = false;
        Status = null;
        Phase = HubPhase.Starting;
        await LaunchHubAsync(newPort ?? _port, persistPort: newPort.HasValue).ConfigureAwait(true);
        IsHubBusy = false;
    }

    public void ShutdownHub()
    {
        if (_isShuttingDown) return;
        _isShuttingDown = true;
        StopPolling();
        _bridge.StopBlocking(TimeSpan.FromSeconds(10));
        _isHubRunning = false;
        _status = null;
        _phase = HubPhase.Offline;
    }

    private async Task LaunchHubAsync(int requestedPort, bool persistPort)
    {
        Phase = HubPhase.Starting;
        LastError = null;
        Notice = null;

        ushort clamped = (ushort)Math.Clamp(requestedPort, 1, 65_535);
        string? logPath = BrowserUsePaths.PrepareLogsDirectory() ? BrowserUsePaths.HubLog : null;

        try
        {
            await _bridge.StartAsync(clamped, logPath, json =>
            {
                DispatchToUi(() => ApplySnapshotJson(json));
            }).ConfigureAwait(true);

            IsHubRunning = true;
            Port = clamped;
            if (persistPort)
            {
                _settings.Port = clamped;
            }

            await RefreshStatusAsync(includeTabs: false).ConfigureAwait(true);
            if (_lastError is null)
            {
                Phase = ExtensionConnected ? HubPhase.HubOnline : HubPhase.WaitingForExtension;
            }
            LastUpdated = DateTimeOffset.Now;
            StartPolling();
        }
        catch (Exception ex)
        {
            IsHubRunning = false;
            Status = null;
            Phase = HubPhase.Offline;
            LastError = ex.Message;
            LastUpdated = DateTimeOffset.Now;
        }
    }

    public void StartPolling()
    {
        StopPolling();
        var cts = new CancellationTokenSource();
        _pollCts = cts;
        _ = PollLoopAsync(cts.Token);
    }

    private void StopPolling()
    {
        _pollCts?.Cancel();
        _pollCts?.Dispose();
        _pollCts = null;
    }

    private async Task PollLoopAsync(CancellationToken token)
    {
        while (!token.IsCancellationRequested)
        {
            try
            {
                await Task.Delay(TimeSpan.FromSeconds(RefreshInterval), token).ConfigureAwait(true);
            }
            catch (OperationCanceledException)
            {
                return;
            }

            if (_isShuttingDown || !_isHubRunning || _isHubBusy) continue;
            await RefreshStatusAsync(includeTabs: ExtensionConnected).ConfigureAwait(true);
        }
    }

    public Task RefreshAsync() => RefreshStatusAsync(includeTabs: ExtensionConnected);

    private async Task RefreshStatusAsync(bool includeTabs)
    {
        if (!_isHubRunning || _isShuttingDown || _isRefreshing) return;
        IsRefreshing = true;
        try
        {
            BrowserUseStatus snapshot = await _bridge.StatusAsync(includeTabs).ConfigureAwait(true);
            if (snapshot.Tabs is null && _status is not null)
            {
                snapshot.Tabs = _status.Tabs;
                snapshot.TabsError = _status.TabsError;
            }
            Status = snapshot;
            LastError = null;
            Phase = snapshot.ExtensionConnected ? HubPhase.HubOnline : HubPhase.WaitingForExtension;
            LastUpdated = DateTimeOffset.Now;
        }
        catch (Exception ex)
        {
            LastError = ex.Message;
            Phase = PhaseForErrorMessage(ex.Message);
            LastUpdated = DateTimeOffset.Now;
        }
        finally
        {
            IsRefreshing = false;
        }
    }

    private void ApplySnapshotJson(string json)
    {
        if (!_isHubRunning || _isShuttingDown) return;
        try
        {
            BrowserUseStatus? snapshot = JsonSerializer.Deserialize<BrowserUseStatus>(
                json,
                new JsonSerializerOptions { PropertyNameCaseInsensitive = true });
            if (snapshot is null) return;

            if (snapshot.Tabs is null && _status is not null)
            {
                snapshot.Tabs = _status.Tabs;
                snapshot.TabsError = _status.TabsError;
            }
            Status = snapshot;
            Phase = snapshot.ExtensionConnected ? HubPhase.HubOnline : HubPhase.WaitingForExtension;
            LastUpdated = DateTimeOffset.Now;

            if (snapshot.ExtensionConnected && !_isRefreshing)
            {
                _ = RefreshStatusAsync(includeTabs: true);
            }
        }
        catch (Exception ex)
        {
            LastError = $"The hub sent an unreadable state snapshot: {ex.Message}";
        }
    }

    public async Task ActivateTabAsync(BrowserUseTab tab)
    {
        try
        {
            string cmd = JsonSerializer.Serialize(new { action = "tab_switch" });
            _ = await _bridge.CommandAsync(cmd, tab.Id).ConfigureAwait(true);
            Notice = $"Switched to {tab.DisplayTitle}.";
            await RefreshStatusAsync(includeTabs: true).ConfigureAwait(true);
        }
        catch (Exception ex)
        {
            Notice = ex.Message;
        }
    }

    public async Task<SecretInfo?> LoadSecretInfoAsync(bool force = false)
    {
        if (!force && _secretInfo is not null) return _secretInfo;
        try
        {
            SecretInfo info = await HubBridge.SecretInfoAsync().ConfigureAwait(true);
            SecretInfo = info;
            return info;
        }
        catch (Exception ex)
        {
            Notice = ex.Message;
            return null;
        }
    }

    public async Task CopySecretAsync()
    {
        SecretInfo? info = await LoadSecretInfoAsync(force: true).ConfigureAwait(true);
        if (info is null) return;
        CopyToClipboard(info.Secret);
        Notice = "Secret copied.";
    }

    public async Task CopyZenSetupCommandAsync()
    {
        SecretInfo? info = await LoadSecretInfoAsync(force: true).ConfigureAwait(true);
        if (info is null) return;
        CopyToClipboard(info.SetupCommand);
        Notice = "Zen setup command copied.";
    }

    public void CopyToClipboard(string text)
    {
        try
        {
            System.Windows.Clipboard.SetText(text);
        }
        catch
        {
            // Ignore clipboard contention errors.
        }
    }

    public void ResetPreferences()
    {
        _settings.Reset();
        OnPropertyChanged(nameof(ShowTabsInMenu));
        OnPropertyChanged(nameof(MenuTabLimit));
        OnPropertyChanged(nameof(RefreshInterval));
        StartPolling();
        Notice = "Preferences reset to defaults.";
    }

    private void SyncTabs(IReadOnlyList<BrowserUseTab> latest)
    {
        Tabs.Clear();
        foreach (BrowserUseTab tab in latest)
        {
            Tabs.Add(tab);
        }
    }

    private static HubPhase PhaseForErrorMessage(string message)
    {
        string lower = message.ToLowerInvariant();
        if (lower.Contains("unauthorized") || lower.Contains("secret"))
        {
            return HubPhase.Unauthorized;
        }
        return HubPhase.Error;
    }

    private static int ResolveInitialPort(int persistedPort)
    {
        string? envPort = Environment.GetEnvironmentVariable("ORANGE_PORT");
        if (!string.IsNullOrWhiteSpace(envPort)
            && int.TryParse(envPort.Trim(), out int parsed)
            && parsed is >= 1 and <= 65_535)
        {
            return parsed;
        }
        return persistedPort;
    }

    private void DispatchToUi(Action action)
    {
        if (_dispatcher is null || _dispatcher.CheckAccess())
        {
            action();
            return;
        }
        _dispatcher.BeginInvoke(action);
    }

    private void NotifyPresentationChanged()
    {
        OnPropertyChanged(nameof(Phase));
        OnPropertyChanged(nameof(PhaseTitle));
        OnPropertyChanged(nameof(StatusPill));
        OnPropertyChanged(nameof(Headline));
        OnPropertyChanged(nameof(HubStateWord));
        OnPropertyChanged(nameof(HubSummary));
        OnPropertyChanged(nameof(UptimeText));
        OnPropertyChanged(nameof(ExtensionVersion));
        OnPropertyChanged(nameof(ProtocolVersion));
        OnPropertyChanged(nameof(ExtensionSubtitle));
        OnPropertyChanged(nameof(HubUrl));
        OnPropertyChanged(nameof(Port));
        OnPropertyChanged(nameof(TabCount));
        OnPropertyChanged(nameof(Peers));
        OnPropertyChanged(nameof(ExtensionConnected));
        OnPropertyChanged(nameof(IsHubBusy));
        OnPropertyChanged(nameof(IsHubRunning));
        OnPropertyChanged(nameof(IsRefreshing));
        OnPropertyChanged(nameof(LastError));
        OnPropertyChanged(nameof(Notice));
        OnPropertyChanged(nameof(ShowSetupCard));
        OnPropertyChanged(nameof(ShowNoticeBanner));
        OnPropertyChanged(nameof(ShowErrorBanner));
        OnPropertyChanged(nameof(PhaseBrush));
        OnPropertyChanged(nameof(PhaseTintBackground));
    }

    private void OnPropertyChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));

    public void Dispose()
    {
        ShutdownHub();
        _bridge.Dispose();
    }
}
