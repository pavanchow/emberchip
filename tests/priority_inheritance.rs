//! Gate 2: priority inheritance bounds priority inversion.
//!
//! The scenario: a low-priority task holds a mutex across a bounded critical
//! section, a high-priority task needs the same mutex, and a medium-priority
//! pure-compute task sits between them.
//!
//! With inheritance the low holder is boosted to the high priority while it
//! holds the mutex, so the medium task cannot run and the high task waits only
//! for the critical section. Without inheritance the medium task preempts the
//! holder, stretching the critical section and the high task's blocking. The
//! test asserts the with-inheritance blocking is bounded by the critical
//! section and is strictly shorter than the without-inheritance blocking.

use emberchip::{workload, Config, Event};

/// Length of low's critical section in the inversion scenario (Op::Compute(6)).
const CRITICAL_SECTION: u64 = 6;

fn blocking_with(inherit: bool) -> u64 {
    let cfg = Config {
        seed: 1,
        priority_inheritance: inherit,
        ..Config::default()
    };
    let mut k = workload::priority_inversion(cfg);
    k.run(60);
    workload::high_blocking_ticks(&k)
}

#[test]
fn inheritance_bounds_high_task_blocking() {
    let with = blocking_with(true);
    assert!(
        with != u64::MAX,
        "with inheritance the high task must eventually acquire the mutex"
    );
    // Bounded by the critical section plus a couple of ticks of scheduling slack.
    assert!(
        with <= CRITICAL_SECTION + 2,
        "with inheritance, high blocked {with} ticks, expected at most {}",
        CRITICAL_SECTION + 2
    );
}

#[test]
fn without_inheritance_inversion_is_worse() {
    let with = blocking_with(true);
    let without = blocking_with(false);
    assert!(
        without > with,
        "without inheritance ({without}) should block high longer than with ({with})"
    );
    // The medium compute burst (12 ticks) leaks into high's blocking window.
    assert!(
        without >= with + 6,
        "without inheritance, medium task should stretch blocking well beyond the \
         critical section: with={with} without={without}"
    );
}

#[test]
fn inheritance_boost_is_logged_and_restored() {
    let cfg = Config {
        seed: 1,
        priority_inheritance: true,
        ..Config::default()
    };
    let mut k = workload::priority_inversion(cfg);
    k.run(60);

    let boosted = k.log.iter().any(|e| {
        matches!(
            e,
            Event::Inherit {
                holder: 0,
                boosted_to: 9,
                ..
            }
        )
    });
    assert!(boosted, "low holder should be boosted to the high priority (9)");

    let restored = k
        .log
        .iter()
        .any(|e| matches!(e, Event::Restore { holder: 0, .. }));
    assert!(restored, "low holder's priority should be restored after unlock");
}

#[test]
fn no_boost_when_inheritance_disabled() {
    let cfg = Config {
        seed: 1,
        priority_inheritance: false,
        ..Config::default()
    };
    let mut k = workload::priority_inversion(cfg);
    k.run(60);
    let boosted = k.log.iter().any(|e| matches!(e, Event::Inherit { .. }));
    assert!(!boosted, "no inheritance events when the feature is off");
}

// ----- nested (transitive) inheritance -----

/// The two nested critical sections in the transitive scenario: low holds m2
/// for 6 ticks, midh holds m2 for 2 ticks. High cannot beat the sum of these.
const NESTED_CHAIN: u64 = 6 + 2;

fn nested_blocking_with(inherit: bool) -> u64 {
    let cfg = Config {
        seed: 1,
        priority_inheritance: inherit,
        ..Config::default()
    };
    let mut k = workload::nested_inversion(cfg);
    k.run(80);
    workload::blocking_ticks(&k, 2)
}

#[test]
fn nested_inheritance_propagates_down_the_chain() {
    // high blocks on midh (holds m1), midh blocks on low (holds m2). The boost
    // from high must reach BOTH midh and, transitively, low.
    let cfg = Config {
        seed: 1,
        priority_inheritance: true,
        ..Config::default()
    };
    let mut k = workload::nested_inversion(cfg);
    k.run(80);

    let midh_boosted = k.log.iter().any(|e| {
        matches!(e, Event::Inherit { holder: 1, boosted_to: 9, .. })
    });
    let low_boosted = k.log.iter().any(|e| {
        matches!(e, Event::Inherit { holder: 0, boosted_to: 9, .. })
    });
    assert!(midh_boosted, "the direct holder midh must be boosted to 9");
    assert!(
        low_boosted,
        "the transitive holder low (two levels down) must be boosted to 9"
    );
    // Both boosts must be restored by the end.
    assert!(k.tasks[0].eff_priority == k.tasks[0].base_priority);
    assert!(k.tasks[1].eff_priority == k.tasks[1].base_priority);
}

#[test]
fn nested_inheritance_bounds_high_blocking() {
    let with = nested_blocking_with(true);
    assert!(with != u64::MAX, "high must acquire the mutex under inheritance");
    // Non-vacuous: high really did block.
    assert!(with > 0, "high never blocked, the nested scenario is vacuous");
    // Bounded by the two nested critical sections plus scheduling slack.
    assert!(
        with <= NESTED_CHAIN + 4,
        "nested inheritance: high blocked {with}, above the chain bound {}",
        NESTED_CHAIN + 4
    );
}

#[test]
fn nested_without_inheritance_is_worse() {
    let with = nested_blocking_with(true);
    let without = nested_blocking_with(false);
    assert!(
        without >= with + 5,
        "without inheritance the noise task should stretch high's blocking well \
         beyond the chain: with={with} without={without}"
    );
}
