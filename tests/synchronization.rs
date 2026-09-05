//! Gate 3: synchronization correctness and determinism.
//!
//!  - Mutual exclusion: two tasks contending for a mutex are never both inside
//!    the protected section on the same tick.
//!  - Semaphores never lose or duplicate a signal.
//!  - Message queues deliver every value once, in order.
//!  - Determinism: the same seed and task set produce a byte-identical event
//!    timeline across independent runs.

use emberchip::{fuzz_ops, workload, Config, Event, Kernel, Op, Rng, Task};

#[test]
fn mutex_never_allows_two_in_critical_section() {
    // Both tasks mark entry and exit of the protected section on the UART. We
    // walk the timeline and assert the section is never occupied by two tasks.
    let cfg = Config {
        seed: 3,
        ..Config::default()
    };
    let mut k = Kernel::new(cfg);
    let m = k.add_mutex();
    for id in 0..2u8 {
        k.add_task(Task::new(id as usize, format!("t{id}"), 5 + id).periodic(
            15,
            vec![
                Op::Compute(1),
                Op::Lock(m),
                Op::Uart(format!("enter{id}\n")),
                Op::Compute(3),
                Op::Uart(format!("exit{id}\n")),
                Op::Unlock(m),
            ],
        ));
    }
    k.run(200);

    // Track the mutex owner over time by replaying lock/unlock from the log.
    let mut owner: Option<usize> = None;
    for ev in &k.log {
        match ev {
            Event::Lock { task, .. } => {
                assert!(owner.is_none(), "two tasks held the mutex at once");
                owner = Some(*task);
            }
            Event::Unlock { task, .. } => {
                assert_eq!(owner, Some(*task), "unlock by a non-owner");
                owner = None;
            }
            _ => {}
        }
    }
    assert!(k.mutexes[m].is_free());
}

#[test]
fn semaphore_conserves_signals() {
    // A producer signals a semaphore N times, a consumer waits N times. No unit
    // is lost or invented: total waits equal total signals and none block
    // forever.
    let cfg = Config {
        seed: 9,
        ..Config::default()
    };
    let mut k = Kernel::new(cfg);
    let s = k.add_semaphore(0, 64);

    let n = 20;
    let mut producer_ops = Vec::new();
    for _ in 0..n {
        producer_ops.push(Op::Compute(1));
        producer_ops.push(Op::SemSignal(s));
    }
    let mut consumer_ops = Vec::new();
    for _ in 0..n {
        consumer_ops.push(Op::SemWait(s));
        consumer_ops.push(Op::Compute(1));
    }
    // Consumer higher priority so it blocks and gets woken by signals.
    k.add_task(Task::new(0, "producer", 3).oneshot(producer_ops));
    k.add_task(Task::new(1, "consumer", 7).oneshot(consumer_ops));
    k.run(300);

    let signals = k
        .log
        .iter()
        .filter(|e| matches!(e, Event::SemSignal { .. }))
        .count();
    let waits = k
        .log
        .iter()
        .filter(|e| matches!(e, Event::SemWait { .. }))
        .count();
    assert_eq!(signals, n, "wrong number of signals");
    assert_eq!(waits, n, "consumer lost or duplicated a signal");
    assert_eq!(k.semaphores[s].count(), 0, "semaphore leaked units");
    assert_eq!(k.tasks[1].jobs_completed, 1, "consumer never finished");
}

#[test]
fn queue_delivers_in_order_without_loss() {
    let cfg = Config {
        seed: 11,
        ..Config::default()
    };
    let mut k = Kernel::new(cfg);
    let q = k.add_queue(4);

    let n = 25u32;
    let mut prod = Vec::new();
    for v in 0..n {
        prod.push(Op::QueueSend(q, v));
        prod.push(Op::Compute(1));
    }
    let mut cons = Vec::new();
    for _ in 0..n {
        cons.push(Op::QueueRecv(q));
    }
    k.add_task(Task::new(0, "producer", 4).oneshot(prod));
    k.add_task(Task::new(1, "consumer", 6).oneshot(cons));
    k.run(400);

    let received: Vec<u32> = k
        .log
        .iter()
        .filter_map(|e| match e {
            Event::QueueRecv { value, .. } => Some(*value),
            _ => None,
        })
        .collect();
    let expected: Vec<u32> = (0..n).collect();
    assert_eq!(received, expected, "queue lost, reordered, or duplicated values");
    assert!(k.queues[q].is_empty());
}

fn run_once(seed: u64) -> Vec<Event> {
    let mut rng = Rng::new(seed);
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
    k.run(800);
    k.log
}

#[test]
fn identical_seed_gives_identical_timeline() {
    let iters = fuzz_ops(50);
    for seed in 0..iters {
        let a = run_once(seed);
        let b = run_once(seed);
        assert!(
            a == b,
            "seed {seed}: two runs diverged, {} vs {} events",
            a.len(),
            b.len()
        );
    }
}

#[test]
fn demo_run_is_deterministic() {
    let a = {
        let mut k = workload::demo(Config {
            seed: 7,
            ..Config::default()
        });
        k.run(120);
        (k.log.clone(), k.mcu.uart.contents().to_string())
    };
    let b = {
        let mut k = workload::demo(Config {
            seed: 7,
            ..Config::default()
        });
        k.run(120);
        (k.log.clone(), k.mcu.uart.contents().to_string())
    };
    assert_eq!(a.0, b.0, "demo timeline diverged");
    assert_eq!(a.1, b.1, "demo UART output diverged");
}
