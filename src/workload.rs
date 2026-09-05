//! Task-set builders: randomized schedulable sets for the correctness gates and
//! hand-built scenarios for the demo and the priority-inheritance contrast.

use crate::kernel::{Config, Kernel, Op, Task};
use crate::rng::Rng;

/// The Liu and Layland utilization bound for rate-monotonic scheduling of `n`
/// tasks: `n * (2^(1/n) - 1)`. A task set with total utilization at or below
/// this bound and rate-monotonic priorities is guaranteed schedulable under a
/// preemptive fixed-priority scheduler.
pub fn rm_bound(n: usize) -> f64 {
    let n = n as f64;
    n * (2f64.powf(1.0 / n) - 1.0)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn lcm(a: u64, b: u64) -> u64 {
    a / gcd(a, b) * b
}

/// Least common multiple of all task periods, the point after which a periodic
/// schedule repeats. Capped so a pathological set cannot blow up a CI run.
pub fn hyperperiod(tasks: &[Task], cap: u64) -> u64 {
    let mut h = 1u64;
    for t in tasks {
        if let Some(p) = t.period {
            h = lcm(h, p);
            if h >= cap {
                return cap;
            }
        }
    }
    h.max(1)
}

const PERIOD_POOL: [u64; 6] = [10, 20, 25, 40, 50, 100];

/// Build a random periodic task set of `n` tasks whose total utilization sits at
/// or below the rate-monotonic bound, with rate-monotonic priorities assigned
/// (shorter period gets higher priority). Each job is pure compute so the
/// scheduler is exercised in isolation. Deterministic in `rng`.
pub fn random_schedulable_set(rng: &mut Rng, n: usize) -> Vec<Task> {
    let n = n.clamp(2, 6);
    let bound = rm_bound(n);

    loop {
        // Pick n distinct periods.
        let mut periods: Vec<u64> = Vec::new();
        let mut guard = 0;
        while periods.len() < n && guard < 1000 {
            let p = *rng.pick(&PERIOD_POOL);
            if !periods.contains(&p) {
                periods.push(p);
            }
            guard += 1;
        }
        if periods.len() < n {
            continue;
        }

        // Give each task a small WCET, then check total utilization.
        let mut wcets: Vec<u64> = Vec::with_capacity(n);
        for &p in &periods {
            let hi = (p / 3).max(1);
            wcets.push(rng.range(1, hi));
        }
        let util: f64 = periods
            .iter()
            .zip(&wcets)
            .map(|(&p, &c)| c as f64 / p as f64)
            .sum();
        if util > bound {
            continue;
        }

        // Rate-monotonic priority: shorter period -> higher priority number.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| periods[i]);
        let mut priority = vec![0u8; n];
        // longest period gets the lowest priority value.
        for (rank, &i) in order.iter().rev().enumerate() {
            priority[i] = (rank as u8) + 1;
        }

        let mut tasks = Vec::with_capacity(n);
        for i in 0..n {
            let t = Task::new(i, format!("t{i}"), priority[i])
                .periodic(periods[i], vec![Op::Compute(wcets[i])]);
            tasks.push(t);
        }
        return tasks;
    }
}

/// The demo scenario: a blinky LED task, a chatty UART task, two periodic tasks
/// that share a mutex, and a high-priority sensor task that preempts. Returns a
/// kernel ready to run.
pub fn demo(config: Config) -> Kernel {
    let mut k = Kernel::new(config);
    let m = k.add_mutex();

    // Priorities are rate-monotonic (shorter period, higher priority) so the
    // set is schedulable. Blinky has the shortest period and the top priority.

    // Blink the LED and print on the UART every 8 ticks.
    k.add_task(
        Task::new(0, "blinky", 6).periodic(
            8,
            vec![
                Op::GpioToggle(crate::mcu::LED_PIN),
                Op::Uart("blink\n".to_string()),
                Op::Compute(1),
            ],
        ),
    );

    // Sensor: short, frequent, preempts the workers.
    k.add_task(
        Task::new(1, "sensor", 5).periodic(
            10,
            vec![Op::Uart("sample\n".to_string()), Op::Compute(1)],
        ),
    );

    // Two periodic workers sharing a mutex-protected section. Worker-a outranks
    // worker-b, so when worker-a waits on the mutex worker-b holds, worker-b is
    // boosted by inheritance.
    k.add_task(
        Task::new(2, "worker-a", 4).periodic(
            20,
            vec![
                Op::Compute(1),
                Op::Lock(m),
                Op::Compute(3),
                Op::Unlock(m),
                Op::Compute(1),
            ],
        ),
    );
    k.add_task(
        Task::new(3, "worker-b", 3).periodic(
            25,
            vec![
                Op::Compute(1),
                Op::Lock(m),
                Op::Compute(2),
                Op::Unlock(m),
            ],
        ),
    );
    k
}

/// The classic three-task priority-inversion scenario, used to show the effect
/// of priority inheritance on and off.
///
/// - `low` (priority 1) takes a mutex and holds it across a long critical
///   section.
/// - `mid` (priority 5) is pure compute and does not touch the mutex.
/// - `high` (priority 9) wakes late, needs the same mutex, and blocks.
///
/// With inheritance, `low` is boosted to `high`'s priority while it holds the
/// mutex, so `mid` cannot run and the blocking of `high` is bounded by the
/// critical section. Without inheritance, `mid` preempts `low`, stretching the
/// critical section and the blocking of `high` without bound.
pub fn priority_inversion(config: Config) -> Kernel {
    let mut k = Kernel::new(config);
    let m = k.add_mutex();

    // low: released at tick 1, grabs the mutex early, long critical section.
    let low = Task::new(0, "low", 1).oneshot(vec![
        Op::Lock(m),
        Op::Compute(6),
        Op::Unlock(m),
        Op::Compute(1),
    ]);
    k.add_task(low);

    // mid: released later, long pure-compute burst, no mutex.
    k.add_task(Task::new(1, "mid", 5).oneshot(vec![Op::Compute(12)]));

    // high: released later still, needs the mutex.
    let high = Task::new(2, "high", 9).oneshot(vec![
        Op::Compute(1),
        Op::Lock(m),
        Op::Compute(2),
        Op::Unlock(m),
    ]);
    k.add_task(high);

    // Phasing so low takes the mutex first, then high arrives to contend, with
    // mid ready in between.
    k.tasks[0].next_release = 1;
    k.tasks[1].next_release = 3;
    k.tasks[2].next_release = 3;
    k
}

/// Measure how long `high` (task id 2) is blocked between the tick it first
/// tries to take the mutex and the tick it acquires it, in the inversion
/// scenario. This is the number the inheritance gate compares.
pub fn high_blocking_ticks(kernel: &Kernel) -> u64 {
    use crate::kernel::Event;
    let mut blocked_at: Option<u64> = None;
    let mut tick = 0u64;
    for ev in &kernel.log {
        match ev {
            Event::Tick(t) => tick = *t,
            Event::BlockOnMutex { task: 2, .. } => blocked_at = Some(tick),
            Event::Lock { task: 2, .. } => {
                if let Some(start) = blocked_at.take() {
                    return tick - start;
                }
            }
            _ => {}
        }
    }
    // Never acquired within the run: treat as maximally blocked.
    if blocked_at.is_some() {
        return u64::MAX;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rm_bound_is_sane() {
        assert!((rm_bound(1) - 1.0).abs() < 1e-9);
        assert!(rm_bound(2) > 0.82 && rm_bound(2) < 0.83);
        assert!(rm_bound(1000) > 0.69);
    }

    #[test]
    fn generated_set_is_within_bound() {
        let mut rng = Rng::new(123);
        for _ in 0..50 {
            let n = (rng.range(2, 6)) as usize;
            let tasks = random_schedulable_set(&mut rng, n);
            let util: f64 = tasks.iter().map(|t| t.utilization()).sum();
            assert!(util <= rm_bound(tasks.len()) + 1e-9, "util {util} over bound");
        }
    }

    #[test]
    fn hyperperiod_caps() {
        let mut rng = Rng::new(1);
        let tasks = random_schedulable_set(&mut rng, 5);
        assert!(hyperperiod(&tasks, 10_000) <= 10_000);
    }
}
