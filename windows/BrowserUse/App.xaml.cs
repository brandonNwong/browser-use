using System.Runtime.InteropServices;
using System.Windows;

namespace BrowserUse;

public partial class App : System.Windows.Application
{
    [DllImport("kernel32.dll")]
    private static extern bool AttachConsole(int dwProcessId);

    private const int AttachParentProcess = -1;
    private const string InstanceMutexName = @"Local\com.browser-use.tray";
    private const string ShowDashboardEventName = @"Local\com.browser-use.tray.show";

    private Mutex? _instanceMutex;
    private EventWaitHandle? _showEvent;
    private RegisteredWaitHandle? _registeredWait;
    private BrowserUseModel? _model;
    private TrayMenuController? _tray;
    private DashboardWindow? _dashboard;
    private SettingsWindow? _settings;

    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        if (HandleHeadlessArguments(e.Args))
        {
            Shutdown(0);
            return;
        }

        bool allowMultiple = e.Args.Contains("--allow-multiple", StringComparer.OrdinalIgnoreCase);
        if (!allowMultiple)
        {
            _instanceMutex = new Mutex(initiallyOwned: true, InstanceMutexName, out bool createdNew);
            if (!createdNew)
            {
                try
                {
                    using var existingShow = EventWaitHandle.OpenExisting(ShowDashboardEventName);
                    existingShow.Set();
                }
                catch
                {
                    // Existing instance may be starting/shutting down.
                }
                Shutdown(0);
                return;
            }

            _showEvent = new EventWaitHandle(false, EventResetMode.AutoReset, ShowDashboardEventName);
            _registeredWait = ThreadPool.RegisterWaitForSingleObject(
                _showEvent,
                (_, _) => Dispatcher.BeginInvoke(OpenDashboard),
                null,
                Timeout.Infinite,
                executeOnlyOnce: false);
        }

        var settingsStore = new BrowserUseSettingsStore();
        _model = new BrowserUseModel(settingsStore, startsHub: true);
        _tray = new TrayMenuController(
            _model,
            openDashboard: OpenDashboard,
            openSettings: OpenSettings,
            quitApp: QuitApp);

        if (e.Args.Contains("--show-dashboard", StringComparer.OrdinalIgnoreCase))
        {
            OpenDashboard();
        }
    }

    protected override void OnExit(ExitEventArgs e)
    {
        _registeredWait?.Unregister(null);
        _showEvent?.Dispose();
        _tray?.Dispose();
        _dashboard?.ForceClose();
        _settings?.ForceClose();
        _model?.Dispose();
        if (_instanceMutex is not null)
        {
            try
            {
                _instanceMutex.ReleaseMutex();
            }
            catch
            {
                // Ignore if not owned.
            }
            _instanceMutex.Dispose();
        }
        base.OnExit(e);
    }

    private void OpenDashboard()
    {
        if (_model is null) return;
        _dashboard ??= new DashboardWindow(_model, OpenSettings);
        if (!_dashboard.IsVisible)
        {
            _dashboard.Show();
        }
        if (_dashboard.WindowState == WindowState.Minimized)
        {
            _dashboard.WindowState = WindowState.Normal;
        }
        _dashboard.Activate();
    }

    private void OpenSettings()
    {
        if (_model is null) return;
        _settings ??= new SettingsWindow(_model);
        if (!_settings.IsVisible)
        {
            _settings.Show();
        }
        if (_settings.WindowState == WindowState.Minimized)
        {
            _settings.WindowState = WindowState.Normal;
        }
        _settings.Activate();
    }

    private void QuitApp() => Shutdown(0);

    private static bool HandleHeadlessArguments(string[] args)
    {
        bool isHeadless = args.Contains("--dump-status-menu", StringComparer.OrdinalIgnoreCase)
            || args.Contains("--login-status", StringComparer.OrdinalIgnoreCase)
            || args.Contains("--register-login", StringComparer.OrdinalIgnoreCase)
            || args.Contains("--unregister-login", StringComparer.OrdinalIgnoreCase);

        if (isHeadless && !Console.IsOutputRedirected)
        {
            AttachConsole(AttachParentProcess);
        }

        if (args.Contains("--dump-status-menu", StringComparer.OrdinalIgnoreCase))
        {
            using var previewModel = BrowserUseModel.CreatePreview();
            using var previewTray = new TrayMenuController(
                previewModel,
                openDashboard: () => { },
                openSettings: () => { },
                quitApp: () => { },
                createTrayIcon: false);
            foreach (string line in previewTray.DumpMenuLines())
            {
                Console.WriteLine(line);
            }
            return true;
        }

        if (args.Contains("--login-status", StringComparer.OrdinalIgnoreCase))
        {
            Console.WriteLine($"login item status: {StartupManager.StatusDescription}");
            return true;
        }

        bool register = args.Contains("--register-login", StringComparer.OrdinalIgnoreCase);
        bool unregister = args.Contains("--unregister-login", StringComparer.OrdinalIgnoreCase);
        if (!register && !unregister)
        {
            return false;
        }

        try
        {
            StartupManager.SetEnabled(register);
            Console.WriteLine(register
                ? "Registered Browser Use as a login item."
                : "Unregistered Browser Use from login items.");
            return true;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Login item change failed: {ex.Message}");
            Environment.Exit(1);
            return true;
        }
    }
}
