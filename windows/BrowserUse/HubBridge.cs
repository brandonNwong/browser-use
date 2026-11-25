using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace BrowserUse;

internal sealed class HubBridgeException(string message) : Exception(message)
{
    public static HubBridgeException NotRunning() =>
        new("The in-process hub is not running. Use Restart Hub to start it.");
}

/// <summary>
/// Thin, safe C# wrapper over the six C-ABI functions exported by <c>browser_use.dll</c>
/// (contract in <c>rust/include/browser_use.h</c>).
///
/// Mirrors <c>macos/Sources/App/BrowserUse/HubBridge.swift</c>:
/// - Every FFI call is serialized through <see cref="FfiGate"/> off the UI thread.
/// - <see cref="StopBlocking"/> joins the Tokio runtime synchronously on exit.
/// - <see cref="BuStateCallback"/> stays rooted until after <c>bu_hub_stop</c> returns.
/// </summary>
internal sealed class HubBridge : IDisposable
{
    private static readonly SemaphoreSlim FfiGate = new(1, 1);

    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };

    private IntPtr _hub;
    private NativeMethods.BuStateCallback? _pinnedCallback;
    private readonly object _callbackLock = new();
    private Action<string>? _stateHandler;

    public async Task StartAsync(ushort port, string? logPath, Action<string> onStateChange)
    {
        await RunSerializedAsync(() =>
        {
            StartLocked(port, logPath, onStateChange);
            return true;
        }).ConfigureAwait(false);
    }

    public async Task StopAsync()
    {
        try
        {
            await RunSerializedAsync(() =>
            {
                StopLocked();
                return true;
            }).ConfigureAwait(false);
        }
        catch
        {
            // Ignore shutdown errors.
        }
    }

    public void StopBlocking(TimeSpan timeout)
    {
        if (!FfiGate.Wait(timeout))
        {
            return;
        }
        try
        {
            StopLocked();
        }
        finally
        {
            FfiGate.Release();
        }
    }

    public async Task<BrowserUseStatus> StatusAsync(bool includeTabs)
    {
        return await RunSerializedAsync(() =>
        {
            if (_hub == IntPtr.Zero)
            {
                throw HubBridgeException.NotRunning();
            }

            IntPtr ptr = NativeMethods.bu_hub_status_json(_hub, includeTabs, out IntPtr errPtr);
            string? err = ConsumeString(errPtr);
            string? json = ConsumeString(ptr);

            if (json is null)
            {
                throw new HubBridgeException(err ?? "Failed to read the hub status.");
            }
            if (err is not null)
            {
                throw new HubBridgeException(err);
            }

            try
            {
                return JsonSerializer.Deserialize<BrowserUseStatus>(json, JsonOptions)
                    ?? throw new HubBridgeException("The hub returned empty status JSON.");
            }
            catch (JsonException ex)
            {
                throw new HubBridgeException($"The hub returned unreadable status JSON: {ex.Message}");
            }
        }).ConfigureAwait(false);
    }

    public async Task<string> CommandAsync(string cmdJson, long? tabId, uint timeoutMs = 0)
    {
        return await RunSerializedAsync(() =>
        {
            if (_hub == IntPtr.Zero)
            {
                throw HubBridgeException.NotRunning();
            }

            byte[] cmdBytes = ToNullTerminatedUtf8(cmdJson);
            IntPtr ptr = NativeMethods.bu_hub_command_json(
                _hub,
                cmdBytes,
                tabId ?? -1,
                timeoutMs,
                out IntPtr errPtr);
            string? err = ConsumeString(errPtr);
            string? result = ConsumeString(ptr);

            if (result is null)
            {
                throw new HubBridgeException(err ?? "The browser command failed.");
            }
            if (err is not null)
            {
                throw new HubBridgeException(err);
            }
            return result;
        }).ConfigureAwait(false);
    }

    public static async Task<SecretInfo> SecretInfoAsync()
    {
        return await RunSerializedAsync(() =>
        {
            IntPtr ptr = NativeMethods.bu_secret_info_json(out IntPtr errPtr);
            string? err = ConsumeString(errPtr);
            string? json = ConsumeString(ptr);

            if (json is null)
            {
                throw new HubBridgeException(err ?? "Failed to load the shared secret.");
            }
            if (err is not null)
            {
                throw new HubBridgeException(err);
            }

            try
            {
                return JsonSerializer.Deserialize<SecretInfo>(json, JsonOptions)
                    ?? throw new HubBridgeException("The hub returned empty secret JSON.");
            }
            catch (JsonException ex)
            {
                throw new HubBridgeException($"The hub returned unreadable secret JSON: {ex.Message}");
            }
        }).ConfigureAwait(false);
    }

    private void StartLocked(ushort port, string? logPath, Action<string> onStateChange)
    {
        StopLocked();

        lock (_callbackLock)
        {
            _stateHandler = onStateChange;
        }

        NativeMethods.BuStateCallback callback = OnNativeStateChange;
        _pinnedCallback = callback;

        byte[]? logBytes = logPath is null ? null : ToNullTerminatedUtf8(logPath);
        IntPtr started = NativeMethods.bu_hub_start(port, logBytes, callback, IntPtr.Zero, out IntPtr errPtr);
        string? err = ConsumeString(errPtr);

        if (started == IntPtr.Zero)
        {
            ClearStateHandler();
            _pinnedCallback = null;
            throw new HubBridgeException(err ?? "The in-process hub failed to start.");
        }

        _hub = started;
    }

    private void StopLocked()
    {
        if (_hub == IntPtr.Zero)
        {
            return;
        }
        IntPtr handle = _hub;
        _hub = IntPtr.Zero;
        NativeMethods.bu_hub_stop(handle);
        ClearStateHandler();
        _pinnedCallback = null;
    }

    private void ClearStateHandler()
    {
        lock (_callbackLock)
        {
            _stateHandler = null;
        }
    }

    private void OnNativeStateChange(IntPtr statusJsonPtr, IntPtr ctx)
    {
        if (statusJsonPtr == IntPtr.Zero)
        {
            return;
        }
        string? json = Marshal.PtrToStringUTF8(statusJsonPtr);
        if (string.IsNullOrEmpty(json))
        {
            return;
        }

        Action<string>? handler;
        lock (_callbackLock)
        {
            handler = _stateHandler;
        }
        handler?.Invoke(json);
    }

    private static Task<T> RunSerializedAsync<T>(Func<T> work)
    {
        return Task.Run(async () =>
        {
            await FfiGate.WaitAsync().ConfigureAwait(false);
            try
            {
                return work();
            }
            finally
            {
                FfiGate.Release();
            }
        });
    }

    private static byte[] ToNullTerminatedUtf8(string value)
    {
        int byteCount = Encoding.UTF8.GetByteCount(value);
        byte[] buffer = new byte[byteCount + 1];
        Encoding.UTF8.GetBytes(value, 0, value.Length, buffer, 0);
        buffer[byteCount] = 0;
        return buffer;
    }

    private static string? ConsumeString(IntPtr ptr)
    {
        if (ptr == IntPtr.Zero)
        {
            return null;
        }
        try
        {
            return Marshal.PtrToStringUTF8(ptr);
        }
        finally
        {
            NativeMethods.bu_free_string(ptr);
        }
    }

    public void Dispose()
    {
        StopBlocking(TimeSpan.FromSeconds(5));
    }

    private static class NativeMethods
    {
        private const string DllName = "browser_use.dll";

        [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
        internal delegate void BuStateCallback(IntPtr statusJson, IntPtr ctx);

        [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern IntPtr bu_hub_start(
            ushort port,
            byte[]? logPath,
            BuStateCallback? onChange,
            IntPtr ctx,
            out IntPtr errOut);

        [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern void bu_hub_stop(IntPtr hub);

        [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern IntPtr bu_hub_status_json(
            IntPtr hub,
            [MarshalAs(UnmanagedType.I1)] bool includeTabs,
            out IntPtr errOut);

        [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern IntPtr bu_hub_command_json(
            IntPtr hub,
            byte[] cmdJson,
            long tabId,
            uint timeoutMs,
            out IntPtr errOut);

        [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern IntPtr bu_secret_info_json(out IntPtr errOut);

        [DllImport(DllName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern void bu_free_string(IntPtr ptr);
    }
}

internal sealed class BrowserUseStatus
{
    public sealed class ExtensionInfoModel
    {
        public string? Version { get; init; }
        public string? Browser { get; init; }

        [JsonPropertyName("protocol")]
        public string? ProtocolVersion { get; init; }

        public long? ConnectedAtMs { get; init; }
    }

    public bool? Ok { get; init; }
    public string? Service { get; init; }
    public string? Mode { get; init; }
    public int? Port { get; init; }
    public int? Pid { get; init; }

    [JsonPropertyName("protocol")]
    public string? ProtocolVersion { get; init; }

    public long? UptimeMs { get; init; }
    public bool ExtensionConnected { get; init; }

    [JsonPropertyName("extension")]
    public ExtensionInfoModel? ExtensionInfo { get; init; }

    public int? Peers { get; init; }
    public List<BrowserUseTab>? Tabs { get; set; }
    public string? TabsError { get; set; }

    [JsonIgnore]
    public IReadOnlyList<BrowserUseTab> TabsOrEmpty => Tabs ?? [];

    [JsonIgnore]
    public int TabCount => Tabs?.Count ?? 0;
}

internal sealed class BrowserUseTab
{
    public long Id { get; init; }
    public long? WindowId { get; init; }
    public bool? Active { get; init; }
    public bool? Pinned { get; init; }
    public int? Index { get; init; }
    public string? Url { get; init; }
    public string? Title { get; init; }

    [JsonIgnore]
    public bool IsActive => Active == true;

    [JsonIgnore]
    public bool IsPinned => Pinned == true;

    [JsonIgnore]
    public string DisplayTitle
    {
        get
        {
            string trimmed = (Title ?? string.Empty).Trim();
            if (!string.IsNullOrEmpty(trimmed))
            {
                return trimmed;
            }
            if (Uri.TryCreate(Url, UriKind.Absolute, out Uri? uri) && !string.IsNullOrEmpty(uri.Host))
            {
                return uri.Host;
            }
            return string.IsNullOrEmpty(Url) ? "Untitled tab" : Url;
        }
    }
}

internal sealed class SecretInfo
{
    public string Secret { get; init; } = string.Empty;
    public string Source { get; init; } = string.Empty;
    public string SetupCommand { get; init; } = string.Empty;
}
