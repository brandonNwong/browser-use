using System.Diagnostics;
using System.IO;

namespace BrowserUse;

/// <summary>
/// Paths the Windows app owns: shared secret, hub log, and README discovery.
/// Mirrors <c>macos/Sources/App/BrowserUse/BrowserUsePaths.swift</c>.
/// </summary>
internal static class BrowserUsePaths
{
    public static string HomeDirectory
    {
        get
        {
            string? home = Environment.GetEnvironmentVariable("HOME");
            if (!string.IsNullOrWhiteSpace(home))
            {
                return home;
            }
            return Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        }
    }

    public static string SecretFile => Path.Combine(HomeDirectory, ".orange-mcp", "secret");

    public static string DisplaySecretFile
    {
        get
        {
            string? home = Environment.GetEnvironmentVariable("HOME");
            return string.IsNullOrWhiteSpace(home)
                ? @"%USERPROFILE%\.orange-mcp\secret"
                : SecretFile;
        }
    }

    public static string LogsDirectory
    {
        get
        {
            string localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            return Path.Combine(localAppData, "browser-use", "logs");
        }
    }

    public static string HubLog => Path.Combine(LogsDirectory, "hub.log");

    public static bool PrepareLogsDirectory()
    {
        try
        {
            Directory.CreateDirectory(LogsDirectory);
            return true;
        }
        catch
        {
            return false;
        }
    }

    public static void OpenLogs()
    {
        if (File.Exists(HubLog))
        {
            Reveal(HubLog);
            return;
        }

        PrepareLogsDirectory();
        OpenWithShell(LogsDirectory);
    }

    public static void OpenReadme()
    {
        foreach (string candidate in ReadmeCandidates())
        {
            if (File.Exists(candidate))
            {
                OpenWithShell(candidate);
                return;
            }
        }

        string appDir = AppContext.BaseDirectory;
        if (!string.IsNullOrEmpty(appDir))
        {
            OpenWithShell(appDir);
        }
    }

    public static string? ExtensionManifestFile
    {
        get
        {
            foreach (string candidate in ExtensionManifestCandidates())
            {
                if (File.Exists(candidate))
                {
                    return candidate;
                }
            }
            return null;
        }
    }

    public static void OpenExtensionFolder()
    {
        if (ExtensionManifestFile is { } manifest)
        {
            Reveal(manifest);
            return;
        }

        string appDir = AppContext.BaseDirectory;
        if (!string.IsNullOrEmpty(appDir))
        {
            OpenWithShell(appDir);
        }
    }

    public static void Reveal(string path)
    {
        try
        {
            if (File.Exists(path) || Directory.Exists(path))
            {
                Process.Start(new ProcessStartInfo
                {
                    FileName = "explorer.exe",
                    Arguments = $"/select,\"{path}\"",
                    UseShellExecute = true
                });
                return;
            }

            string? parent = Path.GetDirectoryName(path);
            if (!string.IsNullOrEmpty(parent) && Directory.Exists(parent))
            {
                OpenWithShell(parent);
            }
        }
        catch
        {
            // Ignore shell launch errors.
        }
    }

    private static IEnumerable<string> ReadmeCandidates()
    {
        string? dir = Path.GetDirectoryName(Environment.ProcessPath ?? AppContext.BaseDirectory);
        for (int i = 0; i < 6 && !string.IsNullOrEmpty(dir); i++)
        {
            yield return Path.Combine(dir, "README.md");
            dir = Directory.GetParent(dir)?.FullName;
        }
    }

    private static IEnumerable<string> ExtensionManifestCandidates()
    {
        string? dir = Path.GetDirectoryName(Environment.ProcessPath ?? AppContext.BaseDirectory);
        for (int i = 0; i < 6 && !string.IsNullOrEmpty(dir); i++)
        {
            yield return Path.Combine(dir, "extension", "manifest.json");
            dir = Directory.GetParent(dir)?.FullName;
        }
    }

    private static void OpenWithShell(string target)
    {
        try
        {
            Process.Start(new ProcessStartInfo
            {
                FileName = target,
                UseShellExecute = true
            });
        }
        catch
        {
            // Ignore shell launch errors.
        }
    }
}
