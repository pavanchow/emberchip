//! Gate 4: response-time analysis predicts the simulator exactly.
//!
//! The simulator releases every periodic task together at tick 1, which is the
//! critical instant. For a synchronous, independent, preemptive fixed-priority
//! task set, exact response-time analysis is necessary and sufficient, so its
//! schedulable / not-schedulable verdict must match the simulated run tick for
//! tick: a set the analysis calls schedulable misses no deadline, and a set it
//! calls unschedulable misses at least one. This is the depth feature checked
//! against ground truth, and it is non-vacuous because both outcomes occur.

use emberchip::{analyze, fuzz_ops, schedulability, workload, Config, Kernel, Rng};

fn simulate(tasks: &[emberchip::Task]) -> u64 {
    let cfg = Config {
        seed: 1,
        ..Config::default()
    };
    let mut k = Kernel::new(cfg);
    for t in tasks {
        k.add_task(t.clone());
    }
    let horizon = workload::hyperperiod(&k.tasks, 5_000).max(1_000);
    k.run(horizon);
    k.total_deadline_misses()
}

#[test]
fn rta_matches_simulation_over_random_sets() {
    let iters = fuzz_ops(400);
    let mut seen_schedulable = 0u64;
    let mut seen_unschedulable = 0u64;

    for seed in 0..iters {
        let mut rng = Rng::new(seed.wrapping_mul(2_246_822_519).wrapping_add(5));
        let n = rng.range(2, 6) as usize;
        let tasks = workload::random_rm_set(&mut rng, n);

        let a = analyze(&tasks);
        let misses = simulate(&tasks);

        if a.schedulable {
            seen_schedulable += 1;
            assert_eq!(
                misses, 0,
                "seed {seed}: RTA said schedulable but the sim missed {misses} deadlines\n{}",
                schedulability::report(&tasks, &a)
            );
        } else {
            seen_unschedulable += 1;
            assert!(
                misses > 0,
                "seed {seed}: RTA said NOT schedulable but the sim missed nothing\n{}",
                schedulability::report(&tasks, &a)
            );
        }
    }

    // Non-vacuity: the generator must produce both outcomes, otherwise one
    // branch of the correspondence is never exercised.
    assert!(
        seen_schedulable > 0 && seen_unschedulable > 0,
        "only one outcome appeared: {seen_schedulable} schedulable, {seen_unschedulable} not"
    );
}

#[test]
fn utilization_bound_is_conservative() {
    // Every set the utilization test accepts must also pass exact RTA. The
    // reverse need not hold, since the utilization bound is only sufficient.
    let iters = fuzz_ops(400);
    for seed in 0..iters {
        let mut rng = Rng::new(seed.wrapping_mul(11_400_714_819).wrapping_add(1));
        let n = rng.range(2, 6) as usize;
        let tasks = workload::random_rm_set(&mut rng, n);
        let a = analyze(&tasks);
        if a.utilization_ok {
            assert!(
                a.schedulable,
                "seed {seed}: within the utilization bound but RTA says unschedulable\n{}",
                schedulability::report(&tasks, &a)
            );
        }
    }
}
