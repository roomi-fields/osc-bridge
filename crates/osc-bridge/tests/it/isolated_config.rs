//! A test never reaches the machine's own account or settings: a session of
//! `common::mcp_session` keeps the client's configuration in a folder of its
//! own, held under a process-wide lock, and given back when it ends.

use crate::common::{catalogue, initialize, temp, texts, tool_call, ConfigHome, FakeSynthHub};
use osc_bridge::online::{account_file, settings_file};
use serde_json::json;

#[test]
fn a_session_keeps_the_account_in_its_own_configuration_folder() {
    let dir = temp("isolated-session");
    let (root, cache) = (dir.path().join("server"), dir.path().join("cache"));
    catalogue(&root);
    let server = FakeSynthHub::start(&root, None);
    let home = ConfigHome::new();
    let requests = [initialize(), tool_call(1, "get_device_docs", json!({ "device": "synth" }))];

    let replies = crate::common::mcp_session_in(&home, &server.base, &cache, &requests);

    assert_eq!(replies.len(), 2, "{replies:?}");
    assert!(texts(&replies[1]).iter().any(|t| t.contains("The synth's notes")), "{}", replies[1]);
    let id = std::fs::read_to_string(home.account()).expect("the account is delivered under the session's folder");
    assert_eq!(id.trim(), "0123456789abcdef0123456789abcdef");
    assert_eq!(account_file().unwrap(), home.account());
}

#[test]
fn the_settings_live_beside_the_account_in_the_session_folder() {
    let home = ConfigHome::new();
    assert_eq!(settings_file().unwrap(), home.settings());
    assert!(settings_file().unwrap().starts_with(home.path()));
    assert_eq!(settings_file().unwrap().parent(), account_file().unwrap().parent());
}

#[test]
fn the_folder_is_removed_and_the_environment_given_back_when_the_guard_drops() {
    let home = ConfigHome::new();
    assert_eq!(std::env::var_os("XDG_CONFIG_HOME").unwrap(), home.path().as_os_str());
    let path = home.path().to_path_buf();
    // Read under the guard's own lock: no other session can move the variables.
    let (before, after) = home.end();
    assert_eq!(after, before, "the variables are given back as the guard found them");
    assert!(!path.exists(), "the folder is removed");
}

/// A configuration folder that cannot be emptied (a read-only subfolder with a file in it), its
/// stuck subfolder, and its path; `None` where permissions do not stop the person (root, non-unix).
#[cfg(unix)]
fn stuck_home() -> Option<(ConfigHome, std::path::PathBuf)> {
    use std::os::unix::fs::PermissionsExt;
    let home = ConfigHome::new();
    let sub = home.path().join("stuck");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("file"), "x").unwrap();
    std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(sub.join("other"), "x").is_ok() {
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        return None;
    }
    Some((home, sub))
}

#[cfg(unix)]
fn release(sub: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(sub, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_removed_fails_the_test_naming_it() {
    let Some((home, sub)) = stuck_home() else { return };
    let path = home.path().to_path_buf();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(home)));
    release(&sub);
    let payload = result.expect_err("a folder left behind fails the test");
    let message = payload.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains(&path.display().to_string()), "{message}");
    std::fs::remove_dir_all(&path).unwrap();
}

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_removed_during_a_panic_does_not_abort_the_process() {
    let Some((home, sub)) = stuck_home() else { return };
    let path = home.path().to_path_buf();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _home = home;
        panic!("a failed test");
    }));
    release(&sub);
    let payload = result.expect_err("the test's own panic goes on");
    assert_eq!(payload.downcast_ref::<&str>().copied(), Some("a failed test"));
    std::fs::remove_dir_all(&path).unwrap();
}

#[test]
fn the_folder_is_removed_when_the_test_panics() {
    let path = std::sync::Mutex::new(None);
    let result = std::panic::catch_unwind(|| {
        let home = ConfigHome::new();
        *path.lock().unwrap() = Some(home.path().to_path_buf());
        panic!("a failed test");
    });
    assert!(result.is_err());
    let path = path.lock().unwrap().take().unwrap();
    assert!(!path.exists(), "the folder is removed after a failed test");
    // The lock was released: a new guard can be taken.
    drop(ConfigHome::new());
}

#[test]
fn two_sessions_in_threads_never_cross() {
    let workers: Vec<_> = (0..4).map(|_| std::thread::spawn(|| {
        let home = ConfigHome::new();
        for _ in 0..20 {
            assert_eq!(std::env::var_os("XDG_CONFIG_HOME").unwrap(), home.path().as_os_str());
            std::thread::yield_now();
        }
    })).collect();
    for w in workers {
        w.join().unwrap();
    }
}
