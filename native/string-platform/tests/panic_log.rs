//! A panic goes into errors.log as "crash" (its own test binary: the panic hook is the whole process's).

use std::sync::Arc;
use string_platform::errors_log::{ErrorLog, log_panics};

#[test]
fn a_panic_is_logged_as_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(ErrorLog::new(dir.path()));
    log_panics(log.clone());
    let caught = std::panic::catch_unwind(|| panic!("the test's own panic"));
    assert!(caught.is_err());
    let text = std::fs::read_to_string(log.path()).unwrap();
    assert!(text.contains(" crash: panicked at "), "{text}");
    assert!(text.contains("the test's own panic"), "{text}");
}
