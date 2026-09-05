//! Gate 1: fixed-priority preemptive correctness.
//!
//! Over many randomized schedulable task sets, verify two things independently
//! of the scheduler's own logic:
//!   1. At every tick the task that ran had the highest base priority among all
//!      tasks that were ready or running at that instant (no lower-priority task
//!      runs while a higher-priority one is ready). These sets use no mutexes,
//!      so effective priority equals base priority and this is a real check.
//!   2. Every periodic deadline was met, which is guaranteed because each set is
//!      generated at or below the rate-monotonic utilization bound.

use emberchip::{fuzz_ops, workload, Config, Kernel, Rng};

/// Independently confirm the running task dominates the ready set by base
/// priority. Ties are allowed (equal priority), only a strictly-higher ready
/// task being passed over is a violation.
fn check_records(k: &Kernel) -> Result<(), String> {
    for rec in &k.records {
        let Some(ran) = rec.ran else { continue };
        let ran_prio = rec
            .ready
            .iter()
            .find(|e| e.id == ran)
            .map(|e| e.base)
            .ok_or_else(|| format!("tick {}: runner {ran} not in ready set", rec.tick))?;
        for e in &rec.ready {
            if e.base > ran_prio {
                return Err(format!(
                    "tick {}: ran task {ran} (prio {ran_prio}) while task {} (prio {}) was ready",
                    rec.tick, e.id, e.base
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn preemptive_priority_invariant_over_random_sets() {
    let iters = fuzz_ops(200);
    for seed in 0..iters {
        let mut rng = Rng::new(seed.wrapping_mul(2_654_435_761).wrapping_add(17));
        let n = rng.range(2, 6) as usize;
        let tasks = workload::random_schedulable_set(&mut rng, n);

        let cfg = Config {
            seed,
            ..Config::default()
        };
        let mut k = Kernel::new(cfg);
        for t in tasks {
            k.add_task(t);
        }
        let horizon = workload::hyperperiod(&k.tasks, 2_000).max(500);
        k.run(horizon);

        check_records(&k).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert_eq!(
            k.invariant_violations(),
            0,
            "seed {seed}: kernel flagged an effective-priority invariant violation"
        );
    }
}

#[test]
fn schedulable_sets_meet_all_deadlines() {
    let iters = fuzz_ops(200);
    for seed in 0..iters {
        let mut rng = Rng::new(seed.wrapping_mul(1_099_511_628_211).wrapping_add(3));
        let n = rng.range(2, 6) as usize;
        let tasks = workload::random_schedulable_set(&mut rng, n);

        let cfg = Config {
            seed,
            ..Config::default()
        };
        let mut k = Kernel::new(cfg);
        for t in tasks {
            k.add_task(t);
        }
        let horizon = workload::hyperperiod(&k.tasks, 2_000).max(500);
        k.run(horizon);

        assert_eq!(
            k.total_deadline_misses(),
            0,
            "seed {seed}: a schedulable set missed a deadline"
        );
        // Sanity: work actually happened.
        let completed: u64 = k.tasks.iter().map(|t| t.jobs_completed).sum();
        assert!(completed > 0, "seed {seed}: no jobs completed");
    }
}

#[test]
fn strictly_higher_priority_task_always_preempts() {
    // A concrete, non-random check: a long low-priority job is preempted the
    // tick a higher-priority job is released.
    let cfg = Config {
        seed: 1,
        ..Config::default()
    };
    let mut k = Kernel::new(cfg);
    k.add_task(emberchip::Task::new(0, "lo", 1).periodic(100, vec![emberchip::Op::Compute(40)]));
    k.add_task(emberchip::Task::new(1, "hi", 9).periodic(100, vec![emberchip::Op::Compute(3)]));
    // release high at tick 5 so low is mid-compute
    k.tasks[1].next_release = 5;
    k.run(12);

    let preempt = k
        .log
        .iter()
        .any(|e| matches!(e, emberchip::Event::Preempt { preempted: 0, by: 1 }));
    assert!(preempt, "high priority release must preempt the running low task");
}
