//! Max-scale stress harness for emberchip.
//!
//! Not part of `cargo test`. Build in release and run one scenario at a time,
//! or `all`. Every scenario drives the kernel hard for a configured number of
//! ticks while checking invariants incrementally over log chunks, so memory
//! stays flat no matter how long the run is.
//!
//! Scenarios:
//!   lockstorm    hundreds of tasks fight over one mutex with inverted
//!                priorities. Measures the worst blocking of the single
//!                top-priority task with inheritance on, then again with it
//!                off, and asserts the on-run blocking stays inside the
//!                critical-section bound while the off-run is strictly worse.
//!   herd         semaphore thundering herd in periodic waves: producers
//!                signal, consumers wait, units are conserved exactly.
//!   queuechurn   small queues driven to full and empty over and over, FIFO
//!                order and conservation checked value by value.
//!   manytasks    hundreds of tasks at every priority from 0 to 255 on a
//!                stable load, the ready and ran snapshots replayed every tick
//!                and every task required to make progress.
//!   determinism  a big mixed scenario run twice from the same seed, the whole
//!                event stream folded into a hash and compared.
//!   deadlock     two tasks circular-wait on two mutexes, the kernel must
//!                contain it: no hang, no corruption, other tasks unaffected.
//!   all          run everything above in order, exit 1 on any failure.
//!
//! Environment knobs (all optional):
//!   EMBERCHIP_STRESS_TICKS    ticks per scenario (default 200_000)
//!   EMBERCHIP_STRESS_TASKS    task count for scaled scenarios (default 300)
//!   EMBERCHIP_STRESS_REPS     sync operations per job (default 400)
//!   EMBERCHIP_STRESS_SEED     base seed (default 1)
//!   EMBERCHIP_STRESS_CHUNK    log flush chunk in ticks (default 4_096)

use emberchip::kernel::BlockReason;
use emberchip::{Config, Event, Kernel, Op, Task};
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::time::Instant;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    env_u64(name, default as u64) as usize
}

/// Runs `k` for `horizon` ticks in chunks, folding events after each chunk so
/// nothing has to be retained. Returns wall-clock seconds.
fn run_chunked(k: &mut Kernel, horizon: u64, mut fold: impl FnMut(&mut Kernel)) -> f64 {
    let chunk = env_u64("EMBERCHIP_STRESS_CHUNK", 4_096);
    let started = Instant::now();
    let mut done = 0u64;
    while done < horizon {
        let step = chunk.min(horizon - done);
        k.run(step);
        done += step;
        fold(k);
        k.log.clear();
        k.records.clear();
        if done % (chunk * 64) == 0 {
            eprintln!(
                "  progress {done}/{horizon} ticks, {:.1}s elapsed",
                started.elapsed().as_secs_f64()
            );
        }
    }
    started.elapsed().as_secs_f64()
}

fn base_config(seed: u64, priority_inheritance: bool) -> Config {
    Config {
        seed,
        priority_inheritance,
        ..Config::default()
    }
}

/// End-of-run consistency: a boosted task must hold a mutex with waiters on it,
/// and the kernel's own invariant counter must be silent.
fn check_priority_consistency(k: &Kernel, scenario: &str, seed: u64) {
    assert_eq!(
        k.invariant_violations(),
        0,
        "{scenario} seed {seed}: kernel flagged an effective-priority violation"
    );
    for t in &k.tasks {
        let holds_contested = t
            .held_mutexes
            .iter()
            .any(|&m| !k.mutexes[m].waiters.is_empty());
        assert!(
            t.eff_priority == t.base_priority || holds_contested,
            "{scenario} seed {seed}: task {} ends boosted to {} with base {} but \
             holds no contested mutex (boost never restored)",
            t.id,
            t.eff_priority,
            t.base_priority
        );
    }
}

/// Per-task blocking tracker driven by the event stream. Tracks the tick a task
/// blocked on a mutex and the worst gap before its next successful lock, plus a
/// watched task (the boss) whose open interval at the end of the run matters.
struct BlockingTracker {
    blocked_at: Vec<Option<u64>>,
    tick: u64,
    worst: u64,
    boss: Option<usize>,
    boss_worst: u64,
    boss_blocks: u64,
    boss_open: bool,
}

impl BlockingTracker {
    fn new(n_tasks: usize, boss: Option<usize>) -> Self {
        Self {
            blocked_at: vec![None; n_tasks],
            tick: 0,
            worst: 0,
            boss,
            boss_worst: 0,
            boss_blocks: 0,
            boss_open: false,
        }
    }

    fn event(&mut self, ev: &Event) {
        match *ev {
            Event::Tick(t) => self.tick = t,
            Event::BlockOnMutex { task, .. } => {
                self.blocked_at[task] = Some(self.tick);
                if Some(task) == self.boss {
                    self.boss_blocks += 1;
                }
            }
            Event::Lock { task, .. } => {
                if let Some(start) = self.blocked_at[task].take() {
                    let wait = self.tick.saturating_sub(start);
                    if wait > self.worst {
                        self.worst = wait;
                    }
                    if Some(task) == self.boss && wait > self.boss_worst {
                        self.boss_worst = wait;
                    }
                }
            }
            _ => {}
        }
    }

    fn finish(&mut self) {
        self.boss_open = match self.boss {
            Some(b) => self.blocked_at[b].is_some(),
            None => false,
        };
    }
}

// ----- lockstorm -----

/// The critical section length of the low holders. The boss, being the top
/// task in the system, can only ever be blocked by the current holder finishing
/// this section (under inheritance the holder cannot be preempted while boss
/// waits), so it is the theoretical inversion bound.
const LOCKSTORM_CS: u64 = 3;

/// One mutex, one boss above everything, a class of low holders that grab the
/// mutex for a fixed critical section, and a class of mid compute hogs that
/// never touch the mutex but create the preemption pressure that turns an
/// unbounded inversion loose when inheritance is off.
///
/// Task counts scale with `n_tasks`, but the periods scale with the counts so
/// total utilization stays near one half regardless of scale: the mutex is
/// genuinely contended (a holder is inside its section a stable fraction of the
/// time) and the boss, sampling the mutex every few ticks, reliably blocks.
fn build_lockstorm(n_tasks: usize, seed: u64, inherit: bool) -> (Kernel, usize) {
    let mut k = Kernel::new(base_config(seed, inherit));
    let m = k.add_mutex();
    let holders = (n_tasks / 20).clamp(3, 12);
    let mids = (n_tasks / 30).clamp(2, 8);
    // Periods scale with count so the class utilizations are constant:
    // holders ~0.30, mids ~0.10, boss 0.15. Total ~0.55, always schedulable.
    let hold_period = (holders as u64) * 10;
    let mid_period = (mids as u64) * 20;
    for i in 0..holders {
        k.add_task(Task::new(i, format!("lo{i}"), 1 + (i % 3) as u8).periodic(
            hold_period,
            vec![Op::Compute(1), Op::Lock(m), Op::Compute(LOCKSTORM_CS), Op::Unlock(m)],
        ));
    }
    for i in 0..mids {
        k.add_task(
            Task::new(holders + i, format!("mid{i}"), 5 + (i % 3) as u8)
                .periodic(mid_period, vec![Op::Compute(2)]),
        );
    }
    // The boss: exactly one task above everything else. Its blocking under
    // inheritance is provably just the current holder's remaining critical
    // section, because the boosted holder outranks every other task.
    let boss = holders + mids;
    k.add_task(
        Task::new(boss, "boss", 10).periodic(
            20,
            vec![Op::Compute(1), Op::Lock(m), Op::Compute(2), Op::Unlock(m)],
        ),
    );
    (k, boss)
}

fn scenario_lockstorm(n_tasks: usize, ticks: u64, seed: u64) {
    // Worst boss blocking under inheritance: the holder's remaining critical
    // section (at most LOCKSTORM_CS ticks) plus one tick of handoff slack.
    let bound = LOCKSTORM_CS + 2;

    let (boss_worst_on, boss_blocks_on, secs_on) = {
        let (mut k, boss) = build_lockstorm(n_tasks, seed, true);
        let mut tr = BlockingTracker::new(k.tasks.len(), Some(boss));
        let secs = run_chunked(&mut k, ticks, |k| {
            for ev in &k.log {
                tr.event(ev);
            }
        });
        tr.finish();
        check_priority_consistency(&k, "lockstorm-on", seed);
        assert!(
            k.tasks[boss].jobs_completed > 0,
            "lockstorm-on: the boss never completed a job"
        );
        (tr.boss_worst, tr.boss_blocks, secs)
    };

    // Non-vacuity: the whole gate is meaningless unless the boss actually
    // contended for the mutex. If it never blocked, the scenario built no
    // inversion to bound and the pass would be empty.
    assert!(
        boss_blocks_on > 0,
        "lockstorm: the boss never blocked on the mutex, the scenario generated no \
         contention and the bound is vacuous (tasks {n_tasks}, seed {seed})"
    );

    let (boss_worst_off, boss_blocks_off, boss_open_off, secs_off) = {
        let (mut k, boss) = build_lockstorm(n_tasks, seed, false);
        let mut tr = BlockingTracker::new(k.tasks.len(), Some(boss));
        let secs = run_chunked(&mut k, ticks, |k| {
            for ev in &k.log {
                tr.event(ev);
            }
        });
        tr.finish();
        assert_eq!(
            k.invariant_violations(),
            0,
            "lockstorm-off: unexpected violation"
        );
        (tr.boss_worst, tr.boss_blocks, tr.boss_open, secs)
    };

    assert!(
        boss_worst_on <= bound,
        "lockstorm: with inheritance the boss blocked {boss_worst_on} ticks, above the \
         critical-section bound {bound}, inversion is unbounded"
    );
    // The off run must also have exercised the contention to be a fair contrast.
    assert!(
        boss_blocks_off > 0,
        "lockstorm-off: the boss never blocked, cannot compare inversion"
    );
    let off_effective = if boss_open_off { u64::MAX } else { boss_worst_off };
    assert!(
        off_effective > boss_worst_on,
        "lockstorm: disabling inheritance did not make the boss's blocking strictly \
         worse (on {boss_worst_on}, off {boss_worst_off}, open {boss_open_off})"
    );
    println!(
        "lockstorm: tasks {n_tasks} ticks {ticks} | boss blocks on/off {boss_blocks_on}/\
         {boss_blocks_off} | worst blocking PI on {boss_worst_on} (bound {bound}) | PI off \
         {boss_worst_off} open {boss_open_off} | {secs_on:.1}s+{secs_off:.1}s"
    );
}

// ----- herd -----

struct HerdFold {
    signals: u64,
    waits: u64,
    sem: usize,
}

impl HerdFold {
    fn event(&mut self, ev: &Event) {
        match *ev {
            Event::SemSignal { sem, .. } if sem == self.sem => self.signals += 1,
            Event::SemWait { sem, .. } if sem == self.sem => self.waits += 1,
            _ => {}
        }
    }

    fn finish(&self, k: &Kernel) {
        // Conservation: every unit produced is consumed or still counted in
        // the semaphore. A blocked waiter holds nothing, it is a pending
        // consumer, so it stays out of the identity.
        let produced = self.signals;
        let accounted = self.waits + k.semaphores[self.sem].count() as u64;
        assert_eq!(
            produced, accounted,
            "herd: units lost or invented, produced {produced} vs accounted {accounted}"
        );
    }
}

fn scenario_herd(n_tasks: usize, reps: usize, ticks: u64, seed: u64) {
    let mut k = Kernel::new(base_config(seed, true));
    let s = k.add_semaphore(0, u32::MAX);
    let producers = (n_tasks / 2).max(1);
    let consumers = (n_tasks / 2).max(1);
    // The wave period is sized from the total compute demand so utilization
    // lands near one third. That keeps the set schedulable at any scale, so the
    // herd churns for the whole horizon instead of collapsing into overload
    // where low-priority producers would (correctly) starve.
    let demand = (producers + consumers) as u64 * reps.max(1) as u64;
    let wave = (demand * 3).max(4 * reps.max(1) as u64);
    for i in 0..producers {
        let mut prog = Vec::with_capacity(reps * 2);
        for _ in 0..reps {
            prog.push(Op::Compute(1));
            prog.push(Op::SemSignal(s));
        }
        k.add_task(Task::new(i, format!("prod{i}"), 1 + (i % 4) as u8).periodic(wave, prog));
    }
    for i in 0..consumers {
        let mut prog = Vec::with_capacity(reps * 2);
        for _ in 0..reps {
            prog.push(Op::SemWait(s));
            prog.push(Op::Compute(1));
        }
        k.add_task(
            Task::new(producers + i, format!("cons{i}"), 6 + (i % 4) as u8).periodic(wave, prog),
        );
    }
    let mut fold = HerdFold { signals: 0, waits: 0, sem: s };
    let secs = run_chunked(&mut k, ticks, |k| {
        for ev in &k.log {
            fold.event(ev);
        }
    });
    fold.finish(&k);
    check_priority_consistency(&k, "herd", seed);
    // Non-vacuity plus progress: the storm must have actually happened and jobs
    // must have completed. Conservation (checked in fold.finish) is the real
    // gate, but it is only meaningful once real traffic flowed.
    assert!(fold.signals > 0 && fold.waits > 0, "herd: no traffic, gate is vacuous");
    let completed: u64 = k.tasks.iter().map(|t| t.jobs_completed).sum();
    assert!(completed > 0, "herd: no job ever completed");
    println!(
        "herd: tasks {n_tasks} reps {reps} ticks {ticks} | signals {} waits {} count {} \
         completed {completed} | {secs:.1}s",
        fold.signals,
        fold.waits,
        k.semaphores[s].count()
    );
}

// ----- queuechurn -----

struct QueueFold {
    expected: Vec<VecDeque<u32>>,
    mismatches: usize,
}

impl QueueFold {
    fn event(&mut self, ev: &Event) {
        match *ev {
            Event::QueueSend { queue, value, .. } => {
                self.expected[queue].push_back(value);
            }
            Event::QueueRecv { queue, value, .. } => match self.expected[queue].pop_front() {
                Some(front) if front == value => {}
                other => {
                    self.mismatches += 1;
                    eprintln!(
                        "queuechurn: queue {queue} FIFO violation, expected {other:?} got {value}"
                    );
                }
            },
            _ => {}
        }
    }

    fn finish(&self, k: &Kernel) {
        assert_eq!(self.mismatches, 0, "queuechurn: FIFO violations seen");
        for (i, q) in k.queues.iter().enumerate() {
            assert_eq!(
                q.sent,
                q.received + q.len() as u64,
                "queuechurn: queue {i} lost or duplicated values"
            );
        }
    }
}

fn scenario_queuechurn(n_tasks: usize, reps: usize, ticks: u64, seed: u64) {
    let mut k = Kernel::new(base_config(seed, true));
    let n_queues = (n_tasks / 8).clamp(2, 16);
    let caps: [usize; 4] = [1, 2, 3, 4];
    // Sized from total demand so the set stays schedulable at any scale and the
    // queues cycle full-to-empty for the whole horizon instead of overloading.
    let demand = 2 * n_queues as u64 * reps.max(1) as u64;
    let wave = (demand * 3).max(4 * reps.max(1) as u64);
    for qi in 0..n_queues {
        let q = k.add_queue(caps[qi % caps.len()]);
        let mut send_prog = Vec::with_capacity(reps);
        for i in 0..reps {
            send_prog.push(Op::QueueSend(q, ((qi as u32) << 24) | i as u32));
        }
        let mut recv_prog = Vec::with_capacity(reps);
        for _ in 0..reps {
            recv_prog.push(Op::QueueRecv(q));
        }
        // Producer is low, consumer high: the queue is driven to full and back
        // to empty over and over from both directions.
        k.add_task(Task::new(2 * qi, format!("qp{qi}"), 3).periodic(wave, send_prog));
        k.add_task(Task::new(2 * qi + 1, format!("qc{qi}"), 8).periodic(wave, recv_prog));
    }
    let mut fold = QueueFold {
        expected: vec![VecDeque::new(); n_queues],
        mismatches: 0,
    };
    let secs = run_chunked(&mut k, ticks, |k| {
        for ev in &k.log {
            fold.event(ev);
        }
    });
    fold.finish(&k);
    check_priority_consistency(&k, "queuechurn", seed);
    // Non-vacuity plus progress. FIFO order and conservation (fold.finish) are
    // the real gates; they only mean something once values actually flowed.
    let (sent, received): (u64, u64) =
        k.queues.iter().fold((0, 0), |(s, r), q| (s + q.sent, r + q.received));
    assert!(sent > 0 && received > 0, "queuechurn: no traffic, gate is vacuous");
    let completed: u64 = k.tasks.iter().map(|t| t.jobs_completed).sum();
    assert!(completed > 0, "queuechurn: no job ever completed");
    println!(
        "queuechurn: queues {n_queues} reps {reps} ticks {ticks} | sent {sent} received \
         {received} | {secs:.1}s"
    );
}

// ----- manytasks -----

fn build_manytasks(n_tasks: usize, seed: u64) -> Kernel {
    let mut k = Kernel::new(base_config(seed, true));
    let mutexes: Vec<usize> = (0..8).map(|_| k.add_mutex()).collect();
    let sems: Vec<usize> = (0..4).map(|_| k.add_semaphore(1, 16)).collect();
    let queues: Vec<usize> = (0..4).map(|i| k.add_queue(1 + i)).collect();
    // Stable load: each task's period is sized from its own wcet so total
    // utilization lands near one half. Every priority from 0 to 255 appears.
    for i in 0..n_tasks {
        let prio = if i == 0 {
            0
        } else if i == n_tasks - 1 {
            255
        } else {
            ((i * 37) % 254 + 1) as u8
        };
        // Each task touches exactly one synchronization object and plays one
        // role in it, so the workload can never construct a cross-object wait
        // cycle: any starvation here would be a kernel defect, not the setup.
        // Senders and receivers are grouped in pairs (i/8) so every queue gets
        // matched production and consumption rates.
        let (wcet, program) = match i % 8 {
            0 => (2u64, vec![Op::Compute(2)]),
            1 | 5 => {
                let m = mutexes[i % mutexes.len()];
                (
                    5,
                    vec![
                        Op::Compute(1),
                        Op::Lock(m),
                        Op::Compute(2),
                        Op::Unlock(m),
                        Op::Compute(1),
                    ],
                )
            }
            2 => {
                let s = sems[i % sems.len()];
                (3, vec![Op::SemWait(s), Op::Compute(1), Op::SemSignal(s)])
            }
            3 => {
                let q = queues[(i / 8) % queues.len()];
                (2, vec![Op::QueueSend(q, i as u32), Op::Compute(1)])
            }
            4 => {
                let q = queues[(i / 8) % queues.len()];
                (2, vec![Op::QueueRecv(q), Op::Compute(1)])
            }
            _ => (1, vec![Op::Compute(1), Op::GpioToggle(i % 8)]),
        };
        let period = (wcet * n_tasks as u64 * 2).max(4);
        k.add_task(Task::new(i, format!("t{i}"), prio).periodic(period, program));
    }
    k
}

fn scenario_manytasks(n_tasks: usize, ticks: u64, seed: u64) {
    let mut k = build_manytasks(n_tasks, seed);
    let secs = run_chunked(&mut k, ticks, |k| {
        // Independent replay of the ready and ran snapshots: the task that ran
        // must be in the snapshot and must carry the snapshot's maximum
        // effective priority. No use of the scheduler's own decision code.
        for rec in &k.records {
            let Some(ran) = rec.ran else { continue };
            let entry = rec.ready.iter().find(|e| e.id == ran).unwrap_or_else(|| {
                panic!("manytasks: tick {} ran {ran} not in ready set", rec.tick)
            });
            let max_eff = rec.ready.iter().map(|e| e.eff).max().unwrap_or(0);
            assert!(
                entry.eff == max_eff,
                "manytasks: tick {} ran task {ran} at eff {} while eff {max_eff} was ready",
                rec.tick,
                entry.eff
            );
        }
    });
    assert_eq!(
        k.invariant_violations(),
        0,
        "manytasks: kernel invariant counter tripped"
    );
    let (sent, received, in_flight): (u64, u64, u64) = k.queues.iter().fold(
        (0, 0, 0),
        |(s, r, f), q| (s + q.sent, r + q.received, f + q.len() as u64),
    );
    assert_eq!(sent, received + in_flight, "manytasks: queue conservation broken");
    for t in &k.tasks {
        assert!(
            t.jobs_completed > 0,
            "manytasks: task {} (prio {}) starved, never completed a job",
            t.id,
            t.base_priority
        );
    }
    let released: u64 = k.tasks.iter().map(|t| t.jobs_released).sum();
    let completed: u64 = k.tasks.iter().map(|t| t.jobs_completed).sum();
    println!(
        "manytasks: tasks {n_tasks} ticks {ticks} | released {released} completed {completed} \
         misses {} | {secs:.1}s",
        k.total_deadline_misses()
    );
}

// ----- determinism -----

fn scenario_determinism(n_tasks: usize, ticks: u64, seed: u64) {
    let hash_run = || {
        let mut k = build_manytasks(n_tasks, seed);
        let mut ev_hasher = DefaultHasher::new();
        let mut rec_hasher = DefaultHasher::new();
        let secs = run_chunked(&mut k, ticks, |k| {
            for ev in &k.log {
                ev.hash(&mut ev_hasher);
            }
            for rec in &k.records {
                rec.hash(&mut rec_hasher);
            }
        });
        (ev_hasher.finish(), rec_hasher.finish(), secs)
    };
    let (h1, r1, s1) = hash_run();
    let (h2, r2, s2) = hash_run();
    assert_eq!(h1, h2, "determinism: event streams diverged");
    assert_eq!(r1, r2, "determinism: scheduling records diverged");
    println!(
        "determinism: tasks {n_tasks} ticks {ticks} | event hash {h1:016x} | {s1:.1}s+{s2:.1}s"
    );
}

// ----- deadlock -----

fn scenario_deadlock(ticks: u64) {
    let mut k = Kernel::new(base_config(7, true));
    let m1 = k.add_mutex();
    let m2 = k.add_mutex();
    // Task a takes m1 first. Task b is released two ticks later at a higher
    // priority, takes m2, then blocks on m1. When a resumes and asks for m2
    // the cycle is closed: a owns m1 and waits on m2, b owns m2 and waits m1.
    k.add_task(
        Task::new(0, "deadlock-a", 5).oneshot(vec![
            Op::Lock(m1),
            Op::Compute(2),
            Op::Lock(m2),
            Op::Compute(2),
            Op::Unlock(m2),
            Op::Unlock(m1),
        ]),
    );
    k.add_task(
        Task::new(1, "deadlock-b", 9).oneshot(vec![
            Op::Lock(m2),
            Op::Compute(2),
            Op::Lock(m1),
            Op::Compute(2),
            Op::Unlock(m1),
            Op::Unlock(m2),
        ]),
    );
    k.tasks[1].next_release = 3;
    k.add_task(Task::new(2, "background", 1).periodic(20, vec![Op::Compute(3)]));

    let half = ticks / 2;
    let mut owners_mid = (None, None);
    let mut sampled = false;
    let secs = run_chunked(&mut k, ticks, |k| {
        if !sampled && k.now() >= half {
            sampled = true;
            owners_mid = (k.mutexes[m1].owner, k.mutexes[m2].owner);
        }
    });
    assert!(
        owners_mid.0.is_some() && owners_mid.1.is_some(),
        "deadlock: cycle never formed"
    );
    let a = &k.tasks[0];
    let b = &k.tasks[1];
    assert!(
        matches!(a.blocked_on, Some(BlockReason::Mutex(_)))
            && matches!(b.blocked_on, Some(BlockReason::Mutex(_))),
        "deadlock: the cycle did not persist, tasks moved unexpectedly"
    );
    assert_eq!(
        (k.mutexes[m1].owner, k.mutexes[m2].owner),
        owners_mid,
        "deadlock: mutex ownership changed after the cycle formed"
    );
    assert!(
        k.tasks[2].jobs_completed > 0,
        "deadlock: background task starved by the cycle"
    );
    assert_eq!(k.invariant_violations(), 0, "deadlock: invariant tripped");
    println!(
        "deadlock: cycle formed and contained | background jobs {} | {secs:.1}s",
        k.tasks[2].jobs_completed
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("all");
    let ticks = env_u64("EMBERCHIP_STRESS_TICKS", 200_000);
    let n_tasks = env_usize("EMBERCHIP_STRESS_TASKS", 300);
    let reps = env_usize("EMBERCHIP_STRESS_REPS", 400);
    let seed = env_u64("EMBERCHIP_STRESS_SEED", 1);

    let started = Instant::now();
    let mut failed = false;
    macro_rules! run {
        ($name:expr, $body:expr) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $body)) {
                Ok(_) => println!("PASS {}", $name),
                Err(_) => {
                    println!("FAIL {}", $name);
                    failed = true;
                }
            }
        };
    }

    match cmd {
        "lockstorm" => run!("lockstorm", scenario_lockstorm(n_tasks, ticks, seed)),
        "herd" => run!("herd", scenario_herd(n_tasks, reps, ticks, seed)),
        "queuechurn" => run!("queuechurn", scenario_queuechurn(n_tasks, reps, ticks, seed)),
        "manytasks" => run!("manytasks", scenario_manytasks(n_tasks, ticks, seed)),
        "determinism" => run!("determinism", scenario_determinism(n_tasks, ticks, seed)),
        "deadlock" => run!("deadlock", scenario_deadlock(ticks)),
        "all" => {
            run!("lockstorm", scenario_lockstorm(n_tasks, ticks, seed));
            run!("herd", scenario_herd(n_tasks, reps, ticks, seed));
            run!("queuechurn", scenario_queuechurn(n_tasks, reps, ticks, seed));
            run!("manytasks", scenario_manytasks(n_tasks, ticks, seed));
            run!("determinism", scenario_determinism(n_tasks, ticks, seed));
            run!("deadlock", scenario_deadlock(ticks));
        }
        "help" | "--help" | "-h" => {
            print_help();
            return;
        }
        other => {
            eprintln!("unknown scenario: {other}");
            print_help();
            std::process::exit(2);
        }
    }

    println!(
        "stress done, wall {:.1}s, result {}",
        started.elapsed().as_secs_f64(),
        if failed { "FAILED" } else { "ALL PASS" }
    );
    if failed {
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "emberchip stress: max-scale stress harness\n\
         \n\
         USAGE: stress [lockstorm|herd|queuechurn|manytasks|determinism|deadlock|all]\n\
         \n\
         ENV:\n\
         \x20 EMBERCHIP_STRESS_TICKS  ticks per scenario (default 200000)\n\
         \x20 EMBERCHIP_STRESS_TASKS  scaled task count (default 300)\n\
         \x20 EMBERCHIP_STRESS_REPS   sync ops per job (default 400)\n\
         \x20 EMBERCHIP_STRESS_SEED   base seed (default 1)\n\
         \x20 EMBERCHIP_STRESS_CHUNK  log flush chunk in ticks (default 4096)"
    );
}
