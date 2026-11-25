//! Ported legacy `test/secret.test.ts` against `secret.rs` directly (the
//! legacy file exercised the CLI; the CLI has its own target, and this file
//! covers the filesystem/permission/race behavior at the module level).
//!
//! All tests mutate process-global `HOME` / `ORANGE_SECRET`, so they serialize
//! on one static mutex and restore the previous environment on drop.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use browser_use::secret::{
    load_secret, load_secret_info, secret_from_environment, secret_path, secrets_match,
};
#[cfg(unix)]
use browser_use::secret::{SECRET_DIR_MODE, SECRET_FILE_MODE};

const TEST_SECRET: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn lock_environment() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Set a throwaway `HOME`, clear `ORANGE_SECRET`, and restore both on drop.
struct EnvGuard {
    previous_home: Option<OsString>,
    previous_userprofile: Option<OsString>,
    previous_secret: Option<OsString>,
}

impl EnvGuard {
    fn isolate(home: &Path) -> Self {
        let guard = Self {
            previous_home: std::env::var_os("HOME"),
            previous_userprofile: std::env::var_os("USERPROFILE"),
            previous_secret: std::env::var_os("ORANGE_SECRET"),
        };
        std::env::set_var("HOME", home);
        std::env::remove_var("ORANGE_SECRET");
        guard
    }

    #[cfg(windows)]
    fn isolate_without_home(userprofile: &Path) -> Self {
        let guard = Self {
            previous_home: std::env::var_os("HOME"),
            previous_userprofile: std::env::var_os("USERPROFILE"),
            previous_secret: std::env::var_os("ORANGE_SECRET"),
        };
        std::env::remove_var("HOME");
        std::env::set_var("USERPROFILE", userprofile);
        std::env::remove_var("ORANGE_SECRET");
        guard
    }

    fn set_environment_secret(&self, secret: &str) {
        std::env::set_var("ORANGE_SECRET", secret);
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }
        match &self.previous_userprofile {
            Some(profile) => std::env::set_var("USERPROFILE", profile),
            None => std::env::remove_var("USERPROFILE"),
        }
        match &self.previous_secret {
            Some(secret) => std::env::set_var("ORANGE_SECRET", secret),
            None => std::env::remove_var("ORANGE_SECRET"),
        }
    }
}

#[cfg(unix)]
fn permissions_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .unwrap_or_else(|error| panic!("metadata {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

#[cfg(unix)]
fn assert_secret_modes(dir: &Path, path: &Path) {
    assert_eq!(permissions_mode(dir), SECRET_DIR_MODE);
    assert_eq!(permissions_mode(path), SECRET_FILE_MODE);
}

#[cfg(not(unix))]
fn assert_secret_modes(_dir: &Path, _path: &Path) {}

/// `~/.orange-mcp` inside the temporary HOME.
fn secret_dir(home: &Path) -> PathBuf {
    home.join(".orange-mcp")
}

// ── Pure helpers ─────────────────────────────────────────────────────────

#[test]
fn secrets_match_is_length_safe_and_constant_time_semantics() {
    assert!(secrets_match(TEST_SECRET, TEST_SECRET));
    assert!(secrets_match("abc", "abc"));
    assert!(!secrets_match("abc", "abd"));
    assert!(!secrets_match("abc", "abcd"));
    assert!(!secrets_match("abcd", "abc"));
    assert!(!secrets_match("abc", ""));
    assert!(!secrets_match("", ""));
    assert!(!secrets_match("abc", "ABC"));
}

#[test]
fn secret_path_is_absolute_under_home() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let _env = EnvGuard::isolate(temp.path());

    let path = secret_path();
    assert!(path.is_absolute());
    assert_eq!(path, temp.path().join(".orange-mcp").join("secret"));
}

// ── Creation + permissions ───────────────────────────────────────────────

#[tokio::test]
async fn creates_0600_secret_inside_0700_directory_and_is_stable() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let _env = EnvGuard::isolate(temp.path());

    let secret = load_secret().await.expect("first load creates the secret");
    assert_eq!(secret.len(), 64, "32 random bytes as hex");
    assert!(secret
        .chars()
        .all(|character| character.is_ascii_hexdigit()));

    let path = secret_path();
    let dir = secret_dir(temp.path());
    assert_secret_modes(&dir, &path);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        secret,
        "the file holds the secret plus one trailing newline"
    );

    // Atomic creation leaves no temp files behind.
    let entries: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, vec!["secret".to_string()]);

    // Second load is stable (no regeneration).
    let again = load_secret().await.expect("second load");
    assert_eq!(again, secret);
    assert!(!secret_from_environment());

    let info = load_secret_info().await.expect("secret info");
    assert_eq!(info.secret, secret);
    assert_eq!(info.source, path.display().to_string());
    assert!(Path::new(&info.source).is_absolute());
    assert_eq!(
        info.setup_command,
        format!(
            "await OrangeBridge.setSecret({})",
            serde_json::to_string(&secret).unwrap()
        )
    );
}

#[tokio::test]
async fn rejects_symlinked_secret_file() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let _env = EnvGuard::isolate(temp.path());

    let dir = secret_dir(temp.path());
    std::fs::create_dir_all(&dir).unwrap();
    let target = temp.path().join("elsewhere-secret");
    std::fs::write(&target, format!("{TEST_SECRET}\n")).unwrap();
    let link = dir.join("secret");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            eprintln!("skipping Windows symlink test without Developer Mode / symlink privilege: {error}");
            return;
        }
        panic!("symlink_file failed unexpectedly: {error}");
    }

    let error = load_secret().await.expect_err("symlinks must be rejected");
    assert!(error.0.contains("symlink"), "unexpected error: {error}");
    assert!(error.0.contains("remove the link"));
    assert!(error.0.contains("browser-use secret"));
}

#[tokio::test]
async fn rejects_non_regular_secret_file() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let _env = EnvGuard::isolate(temp.path());

    // A directory at the secret path is not a regular file.
    std::fs::create_dir_all(secret_dir(temp.path()).join("secret")).unwrap();

    let error = load_secret()
        .await
        .expect_err("directories must be rejected");
    assert!(
        error.0.contains("not a regular file"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn rejects_empty_secret_file() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let _env = EnvGuard::isolate(temp.path());

    let dir = secret_dir(temp.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("secret"), b"").unwrap();

    let error = load_secret()
        .await
        .expect_err("empty files must be rejected");
    assert!(
        error.0.contains("Secret file is empty"),
        "unexpected error: {error}"
    );
    assert!(error.0.contains("regenerate"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_runs_agree_on_one_secret() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let _env = EnvGuard::isolate(temp.path());

    let mut handles = Vec::new();
    for _ in 0..6 {
        handles.push(tokio::spawn(async { load_secret().await }));
    }

    let mut secrets = Vec::new();
    for handle in handles {
        secrets.push(handle.await.expect("task").expect("load_secret"));
    }

    let first = secrets.first().cloned().expect("at least one secret");
    assert!(
        secrets.iter().all(|secret| secret == &first),
        "concurrent creators disagreed: {secrets:?}"
    );
    assert_eq!(
        std::fs::read_to_string(secret_path()).unwrap().trim(),
        first
    );

    // Exactly one file, no `.secret.*.tmp` leftovers.
    let entries: Vec<String> = std::fs::read_dir(secret_dir(temp.path()))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, vec!["secret".to_string()]);
}

// ── ORANGE_SECRET override ───────────────────────────────────────────────

#[tokio::test]
async fn orange_secret_overrides_disk_and_never_creates_the_file() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let env = EnvGuard::isolate(temp.path());
    env.set_environment_secret("env-secret-123");

    assert!(secret_from_environment());
    let secret = load_secret().await.expect("environment secret");
    assert_eq!(secret, "env-secret-123");
    assert!(
        !secret_dir(temp.path()).exists(),
        "ORANGE_SECRET must never create ~/.orange-mcp"
    );

    let info = load_secret_info().await.expect("secret info");
    assert_eq!(info.secret, "env-secret-123");
    assert_eq!(info.source, "environment");
    assert!(info.source != secret_path().display().to_string());
    assert_eq!(
        info.setup_command,
        "await OrangeBridge.setSecret(\"env-secret-123\")"
    );

    // The legacy implementation trims the environment value.
    env.set_environment_secret("  padded-secret  ");
    assert_eq!(load_secret().await.unwrap(), "padded-secret");

    // Whitespace-only means "not set", so the file path is used again.
    env.set_environment_secret("   ");
    assert!(!secret_from_environment());
    let secret = load_secret().await.expect("falls back to the file");
    assert_eq!(secret.len(), 64);
    assert_secret_modes(&secret_dir(temp.path()), &secret_path());
}

#[cfg(windows)]
#[tokio::test]
async fn falls_back_to_userprofile_when_home_is_unset() {
    let _lock = lock_environment();
    let temp = tempfile::tempdir().expect("tempdir");
    let _env = EnvGuard::isolate_without_home(temp.path());

    let path = secret_path();
    assert!(path.is_absolute());
    assert_eq!(path, temp.path().join(".orange-mcp").join("secret"));

    let secret = load_secret().await.expect("create secret under USERPROFILE");
    assert_eq!(secret.len(), 64);
    assert_eq!(load_secret().await.unwrap(), secret);
}
