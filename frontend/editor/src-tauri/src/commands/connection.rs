use crate::state::connection_state::{
    AppConnectionState,
    ConnectionMode,
    ServerConfig,
};
use crate::utils::{add_log, app_data_dir, system_provisioning_dir};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_store::StoreExt;

const STORE_FILE: &str = "connection.json";
const FIRST_LAUNCH_KEY: &str = "setup_completed";
const CONNECTION_MODE_KEY: &str = "connection_mode";
const SERVER_CONFIG_KEY: &str = "server_config";
const LOCK_CONNECTION_KEY: &str = "lock_connection_mode";
const LOGIN_AGREEMENT_KEY: &str = "login_agreement_enabled";
const PROVISIONING_FILE_NAME: &str = "stirling-provisioning.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub mode: ConnectionMode,
    pub server_config: Option<ServerConfig>,
    pub lock_connection_mode: bool,
}

#[tauri::command]
pub async fn get_connection_config(
    _app_handle: AppHandle,
    state: State<'_, AppConnectionState>,
) -> Result<ConnectionConfig, String> {
    // Offline desktop build: ignore stale SaaS/self-hosted settings and expose
    // only the bundled local backend.
    let mode = ConnectionMode::Local;
    let server_config: Option<ServerConfig> = None;
    let lock_connection_mode = true;

    // Update in-memory state
    if let Ok(mut conn_state) = state.0.lock() {
        conn_state.mode = mode.clone();
        conn_state.server_config = server_config.clone();
        conn_state.lock_connection_mode = lock_connection_mode;
    }

    Ok(ConnectionConfig {
        mode,
        server_config,
        lock_connection_mode,
    })
}

#[tauri::command]
pub async fn set_connection_mode(
    app_handle: AppHandle,
    state: State<'_, AppConnectionState>,
    mode: ConnectionMode,
    server_config: Option<ServerConfig>,
    _lock_connection_mode: Option<bool>,
) -> Result<(), String> {
    log::info!("Setting connection mode: {:?}", mode);

    if mode != ConnectionMode::Local || server_config.is_some() {
        return Err("This offline build only supports the bundled local backend".to_string());
    }

    let store = app_handle
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to access store: {}", e))?;

    // If the store is already locked, protect connection_mode, server_config, and the lock
    // flag from being overwritten by any JS-side call.
    // Only allow marking setup_completed and updating auth-related fields.
    let already_locked = store
        .get(LOCK_CONNECTION_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if already_locked {
        log::warn!("set_connection_mode called while lock_connection_mode=true — preserving connection settings, but marking setup as completed");
        // Still allow setup_completed to be written so the onboarding doesn't repeat.
        store.set(FIRST_LAUNCH_KEY, serde_json::json!(true));
        store
            .save()
            .map_err(|e| format!("Failed to save store: {}", e))?;
        return Ok(());
    }

    // Update in-memory state
    if let Ok(mut conn_state) = state.0.lock() {
        conn_state.mode = ConnectionMode::Local;
        conn_state.server_config = None;
        conn_state.lock_connection_mode = true;
    }

    store.set(
        CONNECTION_MODE_KEY,
        serde_json::to_value(&ConnectionMode::Local)
            .map_err(|e| format!("Failed to serialize mode: {}", e))?,
    );

    store.delete(SERVER_CONFIG_KEY);
    store.set(LOCK_CONNECTION_KEY, serde_json::json!(true));

    // Mark setup as completed
    store.set(FIRST_LAUNCH_KEY, serde_json::json!(true));

    store
        .save()
        .map_err(|e| format!("Failed to save store: {}", e))?;

    log::info!("Connection mode saved successfully");
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProvisioningConfig {
    login_agreement_enabled: Option<bool>,
}

fn provisioning_file_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(app_data_dir().join(PROVISIONING_FILE_NAME));

    if let Some(system_dir) = system_provisioning_dir() {
        paths.push(system_dir.join(PROVISIONING_FILE_NAME));
    }

    paths
}

/// A provisioning file should only lock the update-mode UI when it was placed
/// somewhere that requires administrator rights to write to — i.e. the
/// system-wide provisioning dir written by MSI/Intune. A file in the per-user
/// `app_data_dir()` is just a user dropping JSON in their own profile; locking
/// the UI based on that would let any local user permanently disable the
/// Settings selector for themselves with no way back, because the file is
/// deleted after apply but the lock flag persists in the store.
#[cfg(test)]
pub(crate) fn provisioning_path_is_admin_owned(
    provisioning_path: &std::path::Path,
    system_dir: Option<&std::path::Path>,
) -> bool {
    match system_dir {
        Some(dir) => provisioning_path.starts_with(dir),
        None => false,
    }
}

pub fn apply_provisioning_if_present(app_handle: &AppHandle) -> Result<(), String> {
    let provisioning_paths = provisioning_file_paths();
    let provisioning_path = provisioning_paths
        .into_iter()
        .find(|path| path.exists());

    let provisioning_path = match provisioning_path {
        Some(path) => path,
        None => return Ok(()),
    };

    add_log(format!(
        "🧩 Provisioning file detected: {}",
        provisioning_path.display()
    ));

    let raw = fs::read_to_string(&provisioning_path)
        .map_err(|e| format!("Failed to read provisioning file: {}", e))?;
    let parsed: ProvisioningConfig = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse provisioning file: {}", e))?;

    // Login agreement can be provisioned independently of a server URL so it also applies to
    // local, no-login desktop installs. Persist it before the server-URL handling below, which
    // may early-return when no URL is present.
    if let Some(login_agreement_enabled) = parsed.login_agreement_enabled {
        if let Ok(store) = app_handle.store(STORE_FILE) {
            store.set(LOGIN_AGREEMENT_KEY, serde_json::json!(login_agreement_enabled));
            let _ = store.save();
        }
        add_log(format!(
            "🧩 Provisioned login agreement enabled = {}",
            login_agreement_enabled
        ));
    }

    if parsed.login_agreement_enabled.is_none() {
        add_log(
            "⚠️ Provisioning file has no actionable fields (loginAgreement); skipping apply"
                .to_string(),
        );
        return Ok(());
    }

    // Always persist local mode. Remote server provisioning is intentionally
    // unsupported by this personal offline build.
    let store = app_handle
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to access store: {}", e))?;
    store.set(
        CONNECTION_MODE_KEY,
        serde_json::to_value(&ConnectionMode::Local)
            .map_err(|e| format!("Failed to serialize mode: {}", e))?,
    );
    store.delete(SERVER_CONFIG_KEY);
    store.set(LOCK_CONNECTION_KEY, serde_json::json!(true));

    store
        .save()
        .map_err(|e| format!("Failed to save store: {}", e))?;

    if let Ok(mut conn_state) = app_handle.state::<AppConnectionState>().0.lock() {
        conn_state.mode = ConnectionMode::Local;
        conn_state.server_config = None;
        conn_state.lock_connection_mode = true;
    }

    let user_app_data = app_data_dir();
    if provisioning_path.starts_with(&user_app_data) {
        match fs::remove_file(&provisioning_path) {
            Ok(_) => add_log("✅ Provisioning file applied and removed".to_string()),
            Err(err) => add_log(format!(
                "⚠️ Provisioning applied but failed to remove file: {}",
                err
            )),
        }
    } else {
        add_log("ℹ️ Provisioning applied from system location; leaving file in place".to_string());
    }

    Ok(())
}

/// Whether the login agreement was provisioned as enabled. Read by the backend launcher to pass
/// the `-Dlegal.loginAgreement.enabled` flag to the bundled JVM in local desktop mode.
pub fn login_agreement_enabled(app_handle: &AppHandle) -> bool {
    app_handle
        .store(STORE_FILE)
        .ok()
        .and_then(|store| store.get(LOGIN_AGREEMENT_KEY))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[tauri::command]
pub async fn is_first_launch(app_handle: AppHandle) -> Result<bool, String> {
    let store = app_handle
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to access store: {}", e))?;

    let setup_completed = store
        .get(FIRST_LAUNCH_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(!setup_completed)
}

#[tauri::command]
pub async fn reset_setup_completion(app_handle: AppHandle) -> Result<(), String> {
    log::info!("Resetting setup completion flag");

    let store = app_handle
        .store(STORE_FILE)
        .map_err(|e| format!("Failed to access store: {}", e))?;

    // Reset setup completion flag to force SetupWizard on next launch
    store.set(FIRST_LAUNCH_KEY, serde_json::json!(false));

    store
        .save()
        .map_err(|e| format!("Failed to save store: {}", e))?;

    log::info!("Setup completion flag reset successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // Windows-path tests are cfg-gated because `Path::starts_with` is
    // component-wise: on Linux, `"C:\\foo\\bar"` is a SINGLE path component
    // (since backslash is a literal character there, not a separator) so
    // the prefix never matches the way it would on Windows.
    #[cfg(target_os = "windows")]
    #[test]
    fn user_app_data_provisioning_does_not_lock_ui() {
        // A user dropping a provisioning file in their own roaming AppData
        // (per-user, user-writable) must NOT permanently lock the update-mode
        // selector. The file is deleted after apply but the lock flag would
        // persist in the store, locking the user out with no way back.
        let user_path = PathBuf::from(
            "C:\\Users\\alice\\AppData\\Roaming\\Stirling-PDF\\stirling-provisioning.json",
        );
        let system_dir = PathBuf::from("C:\\ProgramData\\Stirling-PDF");

        assert!(!provisioning_path_is_admin_owned(
            &user_path,
            Some(&system_dir),
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn system_provisioning_dir_does_lock_ui() {
        // A provisioning file in ProgramData\Stirling-PDF (or /Library, /etc)
        // requires admin/root rights to write — those locations are how MSI
        // and Intune deliver policy — so locking the UI here is correct.
        let system_path = PathBuf::from(
            "C:\\ProgramData\\Stirling-PDF\\stirling-provisioning.json",
        );
        let system_dir = PathBuf::from("C:\\ProgramData\\Stirling-PDF");

        assert!(provisioning_path_is_admin_owned(
            &system_path,
            Some(&system_dir),
        ));
    }

    #[test]
    fn linux_etc_provisioning_does_lock_ui() {
        let system_path = PathBuf::from("/etc/stirling-pdf/stirling-provisioning.json");
        let system_dir = PathBuf::from("/etc/stirling-pdf");
        assert!(provisioning_path_is_admin_owned(
            &system_path,
            Some(&system_dir),
        ));
    }

    #[test]
    fn linux_home_config_does_not_lock_ui() {
        let user_path =
            PathBuf::from("/home/alice/.config/Stirling-PDF/stirling-provisioning.json");
        let system_dir = PathBuf::from("/etc/stirling-pdf");
        assert!(!provisioning_path_is_admin_owned(
            &user_path,
            Some(&system_dir),
        ));
    }

    #[test]
    fn macos_library_provisioning_does_lock_ui() {
        let system_path = PathBuf::from(
            "/Library/Application Support/Stirling-PDF/stirling-provisioning.json",
        );
        let system_dir =
            PathBuf::from("/Library/Application Support/Stirling-PDF");
        assert!(provisioning_path_is_admin_owned(
            &system_path,
            Some(&system_dir),
        ));
    }

    #[test]
    fn macos_user_library_does_not_lock_ui() {
        let user_path = PathBuf::from(
            "/Users/alice/Library/Application Support/Stirling-PDF/stirling-provisioning.json",
        );
        let system_dir =
            PathBuf::from("/Library/Application Support/Stirling-PDF");
        assert!(!provisioning_path_is_admin_owned(
            &user_path,
            Some(&system_dir),
        ));
    }

    #[test]
    fn no_system_dir_means_no_lock() {
        // Defensive: when the platform has no defined system_provisioning_dir,
        // refuse to lock — the user-AppData file is the only thing we'd be
        // matching against, and that's the case we explicitly want to leave
        // unlocked.
        let user_path = PathBuf::from("/home/alice/.config/Stirling-PDF/stirling-provisioning.json");
        assert!(!provisioning_path_is_admin_owned(&user_path, None));
    }
}
