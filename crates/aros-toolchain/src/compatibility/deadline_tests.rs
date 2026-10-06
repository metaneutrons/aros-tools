//! Deterministic checks for a compatibility phase's shared process budget.

use std::time::{Duration, Instant};

use aros_common::DiagnosticCode;

use super::remaining_phase_timeout;

#[test]
fn expired_phase_rejects_launch_without_a_process_timeout_context() {
    let started = Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
    let error = remaining_phase_timeout(started, Duration::from_secs(2)).unwrap_err();
    let diagnostics = error.diagnostics();
    let diagnostic = &diagnostics.diagnostics[0];
    assert_eq!(diagnostic.code, DiagnosticCode::ProducerCompatibility);
    assert_eq!(
        diagnostic.message,
        "compatibility phase exhausted its explicit deadline before starting the next command",
    );
    assert!(diagnostic.context.is_none());
    assert!(remaining_phase_timeout(Instant::now(), Duration::ZERO).is_err());
}

#[test]
fn successive_commands_share_the_original_phase_budget() {
    let started = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    let timeout = Duration::from_secs(30);
    let first = remaining_phase_timeout(started, timeout).unwrap();
    let second = remaining_phase_timeout(started, timeout).unwrap();
    assert!(first <= Duration::from_secs(29));
    assert!(!second.is_zero());
    assert!(second <= first);
}
