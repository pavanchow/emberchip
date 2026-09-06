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

/// Build a random periodic task set with distinct periods and rate-monotonic
/// priorities, but WCETs drawn wide enough that the set may be schedulable or
/// overloaded. Used to confirm response-time analysis against the simulator
/// across both outcomes. Deterministic in `rng`.
pub fn random_rm_set(rng: &mut Rng, n: usize) -> Vec<Task> {
    let n = n.clamp(2, 6);
    loop {
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

        // WCET up to two thirds of the period, so total load ranges from light
        // to over one and both schedulable and unschedulable sets appear.
        let wcets: Vec<u64> = periods
            .iter()
            .map(|&p| rng.range(1, (2 * p / 3).max(1)))
            .collect();

        // Rate-monotonic priorities: shorter period gets the higher number, all
        // distinct because the periods are distinct.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| periods[i]);
        let mut priority = vec![0u8; n];
        for (rank, &i) in order.iter().rev().enumerate() {
            priority[i] = (rank as u8) + 1;
        }

        return (0..n)
            .map(|i| {
                Task::new(i, format!("t{i}"), priority[i])
                    .periodic(periods[i], vec![Op::Compute(wcets[i])])
            })
            .collect();
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

/// Measure how long a task is blocked between the tick it first tries to take a
/// mutex and the tick it acquires it. `u64::MAX` if it blocked and never
/// acquired within the run, `0` if it never blocked. This is the number the
/// inheritance gates compare.
pub fn blocking_ticks(kernel: &Kernel, task_id: usize) -> u64 {
    use crate::kernel::Event;
    let mut blocked_at: Option<u64> = None;
    let mut tick = 0u64;
    for ev in &kernel.log {
        match ev {
            Event::Tick(t) => tick = *t,
            Event::BlockOnMutex { task, .. } if *task == task_id => blocked_at = Some(tick),
            Event::Lock { task, .. } if *task == task_id => {
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

/// Measure how long `high` (task id 2) is blocked in the flat inversion
/// scenario. Kept for the inheritance gate.
pub fn high_blocking_ticks(kernel: &Kernel) -> u64 {
    blocking_ticks(kernel, 2)
}

/// A nested (transitive) priority-inversion scenario, used to prove inheritance
/// propagates down a chain of held mutexes, not just one level.
///
/// Two mutexes, `m1` and `m2`, and four one-shot tasks:
/// - `low` (priority 1) takes `m2` and holds it across a long critical section.
/// - `midh` (priority 5) takes `m1`, then tries to take `m2`, so it blocks on
///   `low` and holds `m1` while blocked.
/// - `high` (priority 9) tries to take `m1`, so it blocks on `midh`.
/// - `noise` (priority 6) is pure compute and touches no mutex.
///
/// With inheritance the block by `high` walks the chain and boosts `midh` to 9,
/// then `low` to 9 as well (transitive), so `noise` cannot run and `high` waits
/// only for the two nested critical sections. Without inheritance `noise`
/// preempts the unboosted `low`, stretching the chain and the blocking of
/// `high`. The `high` task is id 2 so `high_blocking_ticks` reads it directly.
pub fn nested_inversion(config: Config) -> Kernel {
    let mut k = Kernel::new(config);
    let m1 = k.add_mutex();
    let m2 = k.add_mutex();

    // low: grabs m2 first, long critical section.
    k.add_task(Task::new(0, "low", 1).oneshot(vec![
        Op::Lock(m2),
        Op::Compute(6),
        Op::Unlock(m2),
    ]));
    // midh: grabs m1, then needs m2 which low holds.
    k.add_task(Task::new(1, "midh", 5).oneshot(vec![
        Op::Lock(m1),
        Op::Compute(1),
        Op::Lock(m2),
        Op::Compute(2),
        Op::Unlock(m2),
        Op::Unlock(m1),
    ]));
    // high (id 2): needs m1 which midh holds.
    k.add_task(Task::new(2, "high", 9).oneshot(vec![
        Op::Compute(1),
        Op::Lock(m1),
        Op::Compute(1),
        Op::Unlock(m1),
    ]));
    // noise: mid priority pure compute, the preemption pressure on low.
    k.add_task(Task::new(3, "noise", 6).oneshot(vec![Op::Compute(10)]));

    // Phasing: low first, midh next, then high and noise arrive to contend.
    k.tasks[0].next_release = 1;
    k.tasks[1].next_release = 2;
    k.tasks[2].next_release = 4;
    k.tasks[3].next_release = 4;
    k
}

/// A many-task contention scenario used to check mutual exclusion and the
/// effective-priority scheduling invariant under real contention.
///
/// A single low-priority `holder` (id 0) is released first and alone, so it
/// grabs the mutex and enters a long critical section before anything higher is
/// ready. Then `n` workers at mixed and deliberately equal priorities are
/// released together, several of which also take the mutex, and a top task
/// (highest priority) is released mid critical-section so it blocks on the low
/// holder and triggers inheritance. Equal-priority blocks exercise the
/// tie-breaking in both the scheduler and the mutex waiter queue. The set is
/// kept schedulable so every task makes progress.
pub fn contended_set(config: Config, n: usize) -> Kernel {
    let mut k = Kernel::new(config);
    let m = k.add_mutex();
    let n = n.clamp(4, 40);
    let period = (n as u64) * 10;

    // The low holder: released alone at tick 1, long critical section.
    k.add_task(Task::new(0, "holder", 1).periodic(
        period,
        vec![Op::Lock(m), Op::Compute(4), Op::Unlock(m), Op::Compute(1)],
    ));

    for i in 0..n {
        // Equal priorities in blocks of four, so ties are exercised heavily.
        let prio = 3 + (i / 4) as u8 % 5;
        // Every third worker also contends for the mutex.
        let program = if i % 3 == 0 {
            vec![Op::Compute(1), Op::Lock(m), Op::Compute(2), Op::Unlock(m)]
        } else {
            vec![Op::Compute(2)]
        };
        k.add_task(Task::new(1 + i, format!("w{i}"), prio).periodic(period, program));
    }
    // One unambiguous top task also sharing the mutex.
    let top = 1 + n;
    k.add_task(Task::new(top, "top", 9).periodic(
        period / 2,
        vec![Op::Compute(1), Op::Lock(m), Op::Compute(1), Op::Unlock(m)],
    ));

    // Phasing: the holder is alone at tick 1 and takes the mutex, the crowd and
    // the top task arrive a few ticks later, mid critical-section.
    for t in k.tasks.iter_mut().skip(1) {
        t.next_release = 3;
    }
    k
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
