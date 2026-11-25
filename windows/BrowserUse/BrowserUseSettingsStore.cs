using System.IO;
using System.Text.Json;

namespace BrowserUse;

internal sealed class BrowserUseSettingsData
{
    public int Port { get; set; } = 19455;
    public int RefreshSeconds { get; set; } = 2;
    public bool ShowTabsInMenu { get; set; } = true;
    public int MenuTabLimit { get; set; } = 8;
}

/// <summary>
/// Persisted Windows preferences stored in <c>%APPDATA%\browser-use\settings.json</c>.
/// Mirrors <c>macos/Sources/App/BrowserUse/BrowserUseDefaults.swift</c>.
/// </summary>
internal sealed class BrowserUseSettingsStore
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true
    };

    private readonly string _settingsPath;
    private BrowserUseSettingsData _data;

    public BrowserUseSettingsStore(string? settingsPath = null)
    {
        string appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        _settingsPath = settingsPath ?? Path.Combine(appData, "browser-use", "settings.json");
        _data = Load(_settingsPath);
    }

    public int Port
    {
        get => Math.Clamp(_data.Port, 1, 65_535);
        set
        {
            int clamped = Math.Clamp(value, 1, 65_535);
            if (_data.Port == clamped) return;
            _data.Port = clamped;
            Save();
        }
    }

    public int RefreshSeconds
    {
        get => Math.Max(1, _data.RefreshSeconds);
        set
        {
            int clamped = Math.Max(1, value);
            if (_data.RefreshSeconds == clamped) return;
            _data.RefreshSeconds = clamped;
            Save();
        }
    }

    public bool ShowTabsInMenu
    {
        get => _data.ShowTabsInMenu;
        set
        {
            if (_data.ShowTabsInMenu == value) return;
            _data.ShowTabsInMenu = value;
            Save();
        }
    }

    public int MenuTabLimit
    {
        get => Math.Clamp(_data.MenuTabLimit, 3, 20);
        set
        {
            int clamped = Math.Clamp(value, 3, 20);
            if (_data.MenuTabLimit == clamped) return;
            _data.MenuTabLimit = clamped;
            Save();
        }
    }

    public void Reset()
    {
        _data = new BrowserUseSettingsData();
        Save();
    }

    private static BrowserUseSettingsData Load(string path)
    {
        try
        {
            if (!File.Exists(path))
            {
                return new BrowserUseSettingsData();
            }
            string json = File.ReadAllText(path);
            return JsonSerializer.Deserialize<BrowserUseSettingsData>(json, JsonOptions)
                ?? new BrowserUseSettingsData();
        }
        catch
        {
            return new BrowserUseSettingsData();
        }
    }

    private void Save()
    {
        try
        {
            string? dir = Path.GetDirectoryName(_settingsPath);
            if (!string.IsNullOrEmpty(dir))
            {
                Directory.CreateDirectory(dir);
            }
            string tmp = $"{_settingsPath}.{Environment.ProcessId}.tmp";
            File.WriteAllText(tmp, JsonSerializer.Serialize(_data, JsonOptions));
            File.Move(tmp, _settingsPath, overwrite: true);
        }
        catch
        {
            // Ignore settings write failures.
        }
    }
}
