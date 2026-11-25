//! Shared-secret management.
//!
//! The secret is the only credential that protects the loopback WebSocket. It
//! lives at `~/.orange-mcp/secret`, is generated once with 256 bits of entropy,
//! and is written with 0600 permissions inside a 0700 directory. A value
//! supplied through `ORANGE_SECRET` takes precedence and is never written to
//! disk.
//!
//! Ported 1:1 from the legacy `src/secret.ts`; `ensureSecretPersisted` is gone
//! because there is no launchd service any more.
//!
//! Module dependency rule: this module may only use
//! `std`, `rand`, `subtle` (plus `serde_json` for the `setupCommand` literal).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rand::RngCore;
use subtle::ConstantTimeEq;

/// Directory mode for `~/.orange-mcp`.
pub const SECRET_DIR_MODE: u32 = 0o700;
/// File mode for the secret file.
pub const SECRET_FILE_MODE: u32 = 0o600;
/// Entropy of a generated secret, in bytes (256 bits).
pub const SECRET_BYTES: usize = 32;

/// How many 20 ms polls the loser of a first-run link race performs.
const RACE_POLL_ATTEMPTS: usize = 20;
/// Delay between race polls (legacy `20 * 20ms`).
const RACE_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Error raised by secret loading/creation.
#[derive(Debug, Clone)]
pub struct SecretError(pub String);

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SecretError {}

impl SecretError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Absolute path of the persisted secret file: `$HOME/.orange-mcp/secret`.
pub fn secret_path() -> PathBuf {
    home_dir().join(".orange-mcp").join("secret")
}

fn home_dir() -> PathBuf {
    if let Some(value) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(value);
    }
    #[cfg(windows)]
    {
        if let Some(value) = std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
            return PathBuf::from(value);
        }
        if let (Some(drive), Some(path)) = (
            std::env::var_os("HOMEDRIVE").filter(|value| !value.is_empty()),
            std::env::var_os("HOMEPATH").filter(|value| !value.is_empty()),
        ) {
            let mut combined = PathBuf::from(drive);
            combined.push(path);
            return combined;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The trimmed, non-empty `ORANGE_SECRET` value when the environment supplies
/// the secret.
fn environment_secret() -> Option<String> {
    std::env::var("ORANGE_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// True when `ORANGE_SECRET` is supplying the secret (no file is used).
pub fn secret_from_environment() -> bool {
    environment_secret().is_some()
}

/// Constant-time comparison that also rejects empty / mismatched-length input.
pub fn secrets_match(expected: &str, received: &str) -> bool {
    if received.is_empty() || expected.len() != received.len() {
        return false;
    }
    expected.as_bytes().ct_eq(received.as_bytes()).into()
}

/// Internal read outcome; `NotFound` means "create one", everything else is a
/// user-facing `SecretError` (legacy distinguished `ENOENT` from the rest).
enum ReadSecretError {
    NotFound,
    Failed(SecretError),
}

fn read_secret_file(path: &Path) -> Result<String, ReadSecretError> {
    // lstat, so symlinks are reported instead of silently followed.
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ReadSecretError::NotFound);
        }
        Err(error) => {
            return Err(ReadSecretError::Failed(SecretError::new(format!(
                "Unable to read {}: {error}",
                path.display()
            ))));
        }
    };

    if metadata.file_type().is_symlink() {
        return Err(ReadSecretError::Failed(SecretError::new(format!(
            "Secret path is a symlink: {}. Refusing to follow it; remove the link and run 'browser-use secret'.",
            path.display()
        ))));
    }

    if !metadata.is_file() {
        return Err(ReadSecretError::Failed(SecretError::new(format!(
            "Secret path is not a regular file: {}",
            path.display()
        ))));
    }

    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            return Err(ReadSecretError::Failed(SecretError::new(format!(
                "Unable to read {}: {error}",
                path.display()
            ))));
        }
    };

    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ReadSecretError::Failed(SecretError::new(format!(
            "Secret file is empty: {}. Delete it and run 'browser-use secret' to regenerate.",
            path.display()
        ))));
    }

    Ok(raw.to_string())
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buffer);
    let mut encoded = String::with_capacity(bytes * 2);
    for byte in buffer {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn generate_secret() -> String {
    random_hex(SECRET_BYTES)
}

#[cfg(unix)]
fn set_mode_best_effort(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode_best_effort(_path: &Path, _mode: u32) {
    // On Windows, %USERPROFILE%\.orange-mcp inherits per-user NTFS ACLs.
}

/// Read after a concurrent creator won the hard-link race. A winning writer
/// links the fully-synced temp file into place, so the file is never observed
/// half-written; the retry loop only tolerates transient "empty" states (e.g.
/// a filesystem that reports the link before data) exactly like the legacy
/// implementation.
fn read_secret_after_race(path: &Path) -> Result<String, SecretError> {
    let mut last_error = SecretError::new(format!("Unable to read {}", path.display()));

    for _ in 0..RACE_POLL_ATTEMPTS {
        match read_secret_file(path) {
            Ok(secret) => return Ok(secret),
            Err(ReadSecretError::NotFound) => {
                last_error = SecretError::new(format!("Unable to read {}", path.display()));
            }
            Err(ReadSecretError::Failed(error)) => {
                if !error.0.to_ascii_lowercase().contains("empty") {
                    return Err(error);
                }
                last_error = error;
            }
        }
        std::thread::sleep(RACE_POLL_INTERVAL);
    }

    Err(last_error)
}

/// Create `path` for the first time. Writes a private temp file, fsyncs it and
/// hard-links it into place: `link` fails atomically with `EEXIST`, so a
/// concurrent process can never observe a half-written (empty) secret file.
fn create_secret_file(path: &Path) -> Result<String, SecretError> {
    let dir = path.parent().ok_or_else(|| {
        SecretError::new(format!(
            "Secret path has no parent directory: {}",
            path.display()
        ))
    })?;
    let secret = generate_secret();
    let temp_path = dir.join(format!(
        ".secret.{}.{}.tmp",
        std::process::id(),
        random_hex(8)
    ));

    std::fs::create_dir_all(dir).map_err(|error| {
        SecretError::new(format!("Unable to create {}: {error}", dir.display()))
    })?;
    set_mode_best_effort(dir, SECRET_DIR_MODE);

    let result = (|| -> Result<String, SecretError> {
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(SECRET_FILE_MODE);
            }
            let mut file = options.open(&temp_path).map_err(|error| {
                SecretError::new(format!("Unable to create {}: {error}", temp_path.display()))
            })?;
            file.write_all(format!("{secret}\n").as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|error| {
                    SecretError::new(format!("Unable to write {}: {error}", temp_path.display()))
                })?;
        }
        set_mode_best_effort(&temp_path, SECRET_FILE_MODE);

        match std::fs::hard_link(&temp_path, path) {
            Ok(()) => Ok(secret),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                read_secret_after_race(path)
            }
            Err(error) => Err(SecretError::new(format!(
                "Unable to create {}: {error}",
                path.display()
            ))),
        }
    })();

    // The temp file never survives, whether we won, lost the race or failed.
    let _ = std::fs::remove_file(&temp_path);
    result
}

/// Return the shared secret, creating `~/.orange-mcp/secret` on first use.
/// `ORANGE_SECRET`, when set to a non-empty value, overrides the file entirely.
pub async fn load_secret() -> Result<String, SecretError> {
    if let Some(secret) = environment_secret() {
        return Ok(secret);
    }

    let path = secret_path();
    match read_secret_file(&path) {
        Ok(secret) => Ok(secret),
        Err(ReadSecretError::NotFound) => create_secret_file(&path),
        Err(ReadSecretError::Failed(error)) => Err(error),
    }
}

/// Secret plus provenance, used by `browser-use secret` and the FFI
/// `bu_secret_info_json`.
///
/// `source` is `"environment"` when `ORANGE_SECRET` supplied the value, else
/// the absolute secret file path as a string.
/// `setup_command` is `await OrangeBridge.setSecret(<json string>)`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretInfo {
    pub secret: String,
    pub source: String,
    pub setup_command: String,
}

/// Load or create the secret together with its provenance.
pub async fn load_secret_info() -> Result<SecretInfo, SecretError> {
    let secret = load_secret().await?;
    let source = if secret_from_environment() {
        "environment".to_string()
    } else {
        secret_path().display().to_string()
    };
    let quoted = serde_json::to_string(&secret)
        .map_err(|error| SecretError::new(format!("Unable to encode secret: {error}")))?;
    let setup_command = format!("await OrangeBridge.setSecret({quoted})");
    Ok(SecretInfo {
        secret,
        source,
        setup_command,
    })
}
