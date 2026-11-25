using Microsoft.Win32;

namespace BrowserUse;

/// <summary>
/// Manages Windows launch-at-login via <c>HKCU\Software\Microsoft\Windows\CurrentVersion\Run</c>.
/// Mirrors <c>macos/Sources/App/Shell/ShellSettings.swift</c>.
/// </summary>
internal static class StartupManager
{
    private const string RunKeyPath = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string ValueName = "Browser Use";

    public static bool IsEnabled
    {
        get
        {
            try
            {
                using RegistryKey? key = Registry.CurrentUser.OpenSubKey(RunKeyPath, writable: false);
                string? raw = key?.GetValue(ValueName) as string;
                return !string.IsNullOrWhiteSpace(raw);
            }
            catch
            {
                return false;
            }
        }
    }

    public static string StatusDescription => IsEnabled ? "enabled" : "disabled";

    public static void SetEnabled(bool enabled)
    {
        using RegistryKey key = Registry.CurrentUser.CreateSubKey(RunKeyPath, writable: true)
            ?? throw new InvalidOperationException("Unable to open the Windows Run registry key.");

        if (enabled)
        {
            string exePath = Environment.ProcessPath
                ?? throw new InvalidOperationException("Unable to determine the Browser Use executable path.");
            key.SetValue(ValueName, $"\"{exePath}\"", RegistryValueKind.String);
        }
        else
        {
            key.DeleteValue(ValueName, throwOnMissingValue: false);
        }
    }
}
