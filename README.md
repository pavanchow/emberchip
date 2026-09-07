# Emberchip

<img src="docs/logo.svg" alt="Emberchip logo" width="96">

A deterministic RTOS simulator on a small emulated microcontroller, written in pure Rust std.

Live playground: https://pavanchow.github.io/emberchip/

## What this is, honestly

A real embedded operating system runs as firmware on a microcontroller, a physical chip with its own CPU, memory map, and peripherals. Emberchip is not that. Emberchip is a faithful, deterministic simulator of one. It models the mechanisms a real-time operating system is built from and runs them on a tiny emulated MCU, all in ordinary Rust std on your host machine.

It is a teaching-accurate model, not firmware you can flash. Nothing here talks to real hardware. What it does do is reproduce the behavior that matters when you are learning how an RTOS actually works: a preemptive fixed-priority scheduler driven by a tick timer, interrupts and interrupt service routines, mutexes with priority inheritance, counting semaphores, message queues, task delays, and memory-mapped peripherals (GPIO with an LED, a UART, a timer).

Because the whole thing is a pure function of a seed and a task set, every run is exactly reproducible. That determinism is what lets the correctness gates make hard claims and check them.

## The gap it fills

Learning RTOS concepts from a real board is noisy. Timing jitter, a debugger that changes the timing it measures, and a hundred vendor-specific registers all sit between you and the idea you are trying to understand. Reading a textbook is the opposite problem: the mechanism is described but you cannot poke it.

Emberchip sits in the middle. You can watch a high-priority task preempt a low-priority one on a real timeline, toggle priority inheritance on and off and see unbounded priority inversion appear, and step a mutex-contention scenario tick by tick. The model is small enough to read end to end and precise enough that the scheduling theory (rate-monotonic bounds, priority inheritance) shows up in the output exactly as the theory predicts.

## Quickstart

```
cargo run -- demo
cargo run -- run 300 --seed 7
cargo run -- inversion on
cargo run -- inversion off
cargo run -- analyze --seed 7
```

- `demo` runs a blinky LED task, two periodic workers sharing a mutex, and a high-priority sensor task that preempts them. It prints the scheduling timeline, GPIO and UART activity, and the per-task summary.
- `run [TICKS]` generates a random schedulable task set (utilization within the rate-monotonic bound) and runs it, reporting invariant violations and deadline misses (both should be zero).
- `inversion on|off` runs the three-task priority-inversion scenario with priority inheritance enabled or disabled, and reports how long the high-priority task was blocked.
- `analyze` runs rate-monotonic schedulability analysis on a random task set. It prints the utilization bound test and the exact per-task worst-case response times, gives a schedulable or not verdict, then simulates the set and confirms the analysis matches the run.

Add `--quiet` to print only the summary, or `--seed N` to change the seed.

## API sketch

```rust
use emberchip::{Config, Kernel, Op, Task};

let mut k = Kernel::new(Config::default());
let m = k.add_mutex();

k.add_task(Task::new(0, "blinky", 2).periodic(
    8,
    vec![Op::GpioToggle(emberchip::mcu::LED_PIN), Op::Uart("blink\n".into()), Op::Compute(1)],
));
k.add_task(Task::new(1, "worker", 5).periodic(
    20,
    vec![Op::Compute(1), Op::Lock(m), Op::Compute(3), Op::Unlock(m)],
));

k.run(100);
println!("{}", emberchip::timeline::render(&k));
```

A task is described by a program: a list of `Op`s it performs each job. `Compute(n)` burns CPU ticks (this is the worst-case execution time the schedulability model counts), and the rest touch peripherals or synchronization primitives. The scheduler runs the highest-priority ready task one quantum at a time. Priority is a `u8` where a larger number is more urgent.

## The correctness gates

The gates are committed as tests and run in CI. Set `EMBERCHIP_FUZZ_OPS` to widen the randomized runs (for example `EMBERCHIP_FUZZ_OPS=2000 cargo test`).

1. **Fixed-priority preemptive correctness** (`tests/preemption.rs`). Over many randomized task sets, at every tick the task that ran had the highest priority among all ready tasks, checked independently of the scheduler from a recorded snapshot of the ready set. For sets generated at or below the rate-monotonic utilization bound, every periodic deadline is met. A further case drives many tasks (including deliberately equal priorities) into real contention over one mutex and checks, again from the snapshots alone, that the running task always carries the maximum effective priority and that the mutex is never doubly held.
2. **Priority inheritance bounds inversion** (`tests/priority_inheritance.rs`). In a low, medium, high task scenario sharing a mutex, with inheritance the high task is blocked no longer than the low task's bounded critical section. With inheritance off, the medium task stretches the blocking well beyond the critical section. The test asserts both, so the contrast is proven, not asserted. A nested case with a two-mutex chain proves the boost propagates two levels down to the indirect holder and is then restored.
3. **Synchronization correctness and determinism** (`tests/synchronization.rs`). Mutexes give mutual exclusion (never two tasks in the protected section at once), semaphores never lose or duplicate a signal, message queues deliver every value once and in order, and the same seed and task set produce a byte-identical event timeline across runs.
4. **Schedulability analysis matches the simulator** (`tests/schedulability.rs`). Exact response-time analysis predicts, for each random task set, whether it is schedulable. Because the simulator releases every task together at the critical instant, that analysis is exact, so the gate confirms its verdict matches the simulated run on both outcomes, and that the utilization bound is conservative. Every gate asserts the contention or the outcome it depends on actually occurred, so none can pass vacuously.

Every module also carries unit tests: scheduler pick, mutex, semaphore, queue, timer, ISR dispatch, and response-time analysis.

## Stress harness

`src/bin/stress.rs` is a separate max-scale harness, not part of `cargo test`. It drives the kernel hard while checking invariants incrementally over log chunks, so memory stays flat no matter how long the run is. Scenarios cover a lock storm that measures the inheritance blocking bound against a strictly worse no-inheritance run, a semaphore thundering herd checked for conservation, queue churn checked for FIFO and conservation, hundreds of mixed-priority tasks replayed against the scheduling invariant every tick, a determinism hash, and a contained deadlock. Sizes are read from environment knobs (`EMBERCHIP_STRESS_TICKS`, `EMBERCHIP_STRESS_TASKS`, `EMBERCHIP_STRESS_REPS`, `EMBERCHIP_STRESS_SEED`) and default to bounded values.

```
cargo run --release --bin stress -- all
EMBERCHIP_STRESS_TICKS=40000 EMBERCHIP_STRESS_TASKS=200 cargo run --release --bin stress -- lockstorm
```

## Layout

```
src/
  mcu/        emulated MCU: memory, GPIO, UART, timer, interrupt controller
  kernel/     tasks, scheduler, mutex, semaphore, queue, event log
  rng.rs      seeded PRNG (SplitMix64)
  timeline.rs event log rendering
  workload.rs random schedulable sets and demo scenarios
  schedulability.rs  rate-monotonic utilization bound and response-time analysis
  bin/emberchip.rs   the CLI
  bin/stress.rs      the max-scale stress harness
tests/        the four correctness gates
docs/index.html      the browser playground
```

See `DESIGN.md` for the emulated MCU, the task model, the scheduler, priority inheritance, the synchronization primitives, ISRs, and why each gate proves its claim.

## License

MIT.
