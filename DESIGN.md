# Emberchip design

This document explains the emulated microcontroller, the task model, the preemptive scheduler, priority inheritance, the synchronization primitives, and interrupt handling, and then why each correctness gate proves what it claims.

Emberchip is a deterministic simulator, not firmware. It runs in pure Rust std on the host. The point of the design is to model the real mechanisms of a real-time operating system accurately enough that the behavior you observe matches the theory, while staying small enough to read.

## Time and determinism

Time is discrete. The unit is a tick. One call to `Kernel::step` advances the clock by one tick and does everything that happens in that tick: the timer raises its interrupt, the tick ISR runs, the scheduler picks a task, and that task runs for one quantum.

The entire simulation is a pure function of its configuration (a seed, the RAM size, the timer reload, and whether priority inheritance is on) and the task set. There is no wall-clock time, no threads, and no OS scheduling involved. The only randomness comes from a seeded SplitMix64 generator in `src/rng.rs`, so a given seed reproduces the same run exactly, event for event. That property is what the determinism gate checks and what makes every other gate a hard claim rather than a hope.

## The emulated MCU

The chip lives in `src/mcu/` and is a plain struct, `Mcu`, holding memory, peripherals, an interrupt controller, and a cycle counter.

- **Memory** (`memory.rs`) is a flat byte-addressable RAM with byte and little-endian word access and bounds checks. Task stacks live here as ordinary data, which is exactly how a real kernel sees a stack: a region of memory a task reads and writes. Each task reserves a stack region.
- **GPIO** (`gpio.rs`) is a small port of boolean pins. Pin 0 is wired to the on-board LED, so a task toggling pin 0 makes the LED blink in the timeline and in the playground.
- **UART** (`uart.rs`) is output only. Writes append to a transmit buffer the host can read back, standing in for bytes leaving the chip on a wire.
- **Timer** (`timer.rs`) counts down from a reload value and raises an interrupt when it wraps. With a reload of 1 it fires every clock, which is the system tick that drives scheduling.
- **Interrupt controller** (`interrupt.rs`) latches raised IRQ lines as pending and hands them to the kernel to service, highest vector priority first (timer over UART over GPIO). This models the hardware path from a peripheral event to a software handler.

`Mcu::cycle` advances the hardware one clock and lets the timer raise its interrupt. The kernel calls it once per tick.

## The task model

A task (`src/kernel/task.rs`) is described by a program: a list of operations it performs on each job. This is the key modeling choice. Rather than run real machine code, a task declares what it does, and the kernel executes that declaration under the scheduler.

The operations are `Compute(n)` (burn n ticks of CPU, the work the schedulability model counts), `Delay(n)` (sleep n ticks), GPIO and UART writes, mutex lock and unlock, semaphore wait and signal, and queue send and receive.

Each task has a fixed base priority (a `u8`, larger is more urgent), an optional period, a worst-case execution time computed from the sum of its `Compute` steps, one of four states (ready, running, blocked, suspended), a program counter, and bookkeeping for releases, completions, missed deadlines, and CPU ticks consumed. Periodic tasks are released every period with an implicit deadline equal to the period. One-shot tasks are released once.

### How a quantum runs

When the scheduler selects a task, `run_quantum` steps its program forward. Peripheral writes and non-blocking synchronization are instantaneous and chain within the same quantum. A `Compute` step consumes exactly one CPU tick and then yields. A blocking operation (a contended lock, an empty semaphore, an empty or full queue, or a delay) parks the task and yields the tick. When the last `Compute` of a job finishes, the job completes on that same tick, so a job costs exactly its worst-case execution time in CPU ticks and never an extra tick to notice it is done. That exactness is what lets the rate-monotonic bound hold in the model.

## The preemptive fixed-priority scheduler

The scheduler is the core. On every tick, after the tick ISR has released due jobs and woken sleepers, the scheduler picks the ready task with the highest effective priority, breaking ties by the lowest task id for determinism. If that task is not the one that was running, and the previous task was still runnable, the previous task is preempted and returned to the ready state. The chosen task then runs one quantum.

This is preemptive fixed-priority scheduling. A higher-priority task that becomes ready always displaces a lower-priority running task at the next tick boundary. Because releases happen in the tick ISR before the scheduling decision, a task released at tick t can preempt at tick t.

For a periodic task set with rate-monotonic priorities (shorter period gets higher priority) and total utilization at or below the Liu and Layland bound `n * (2^(1/n) - 1)`, preemptive fixed-priority scheduling guarantees every deadline is met. Emberchip generates random sets under that bound and checks exactly this.

A periodic job released at tick r with period p owns exactly p ticks, from r to r+p-1, because its successor is due at r+p. A job that has not finished by tick r+p has missed its deadline, and the tick ISR flags it there. Getting this boundary right matters: granting the job one extra tick would let an over-demanded job finish at r+p, overlap its next release, and hide a real miss.

## Schedulability analysis

`src/schedulability.rs` adds two classic offline tests for a periodic task set, so schedulability can be predicted before the set is ever run.

The first is the Liu and Layland utilization bound above. It is sufficient but not necessary: a set can fail it and still meet every deadline.

The second is exact response-time analysis. The worst-case response time of a task is the fixed point of `R = C + sum over higher-priority tasks j of ceil(R / T_j) * C_j`, iterated from `R = C` upward until it stops growing or crosses the deadline. The task is schedulable exactly when its response time is within its deadline. For a synchronous, independent, preemptive fixed-priority task set this is both necessary and sufficient.

The simulator releases every periodic task together at tick 1, which is precisely the critical instant response-time analysis assumes. That makes the analysis exact for what the simulator runs, so its verdict must match the simulated outcome tick for tick. Gate 4 checks exactly that correspondence in both directions.

## Priority inheritance

A mutex (`src/kernel/mutex.rs`) tracks its owner and a queue of blocked waiters. Plain priority-based mutual exclusion has a well known failure: priority inversion. A low-priority task holds a mutex, a high-priority task needs it and blocks, and then a medium-priority task that does not touch the mutex preempts the low holder. The high task now waits not just for the short critical section but for all the medium work, without bound.

Priority inheritance fixes this. When a task blocks on a mutex, the kernel boosts the holder's effective priority to that of the highest-priority waiter, so the holder cannot be preempted by anything below that waiter. The boost applies along the chain, so a holder that is itself blocked on another mutex propagates the boost onward. On release, the holder's effective priority is recomputed from its base and any tasks still waiting on mutexes it holds, dropping the boost when it is no longer justified. The mutex is then handed directly to the highest-priority waiter.

When inheritance is turned off in the configuration, mutexes still give mutual exclusion but no boosting happens, which reproduces unbounded inversion. Emberchip runs the same scenario both ways and measures the difference.

## Synchronization primitives

- **Mutex** gives mutual exclusion with priority inheritance, as above. It is not recursive. Release grants the mutex to the highest-priority waiter.
- **Semaphore** (`semaphore.rs`) is a counting semaphore. A wait takes a unit or blocks. A signal either hands a unit straight to the highest-priority waiter or increments the count, saturating at a maximum. A unit is never lost or duplicated: every signal either satisfies exactly one waiter or bumps the count by exactly one.
- **Message queue** (`queue.rs`) is bounded and strict FIFO. Sends block when full, receives block when empty. Values come out in the order they went in with no loss and no duplication. When a send frees space or delivers a value, a blocked partner is woken and makes progress on its next quantum.

All three wake the highest-priority waiter first, with the lowest id breaking ties, so wakeups are deterministic.

## Interrupts and ISRs

The timer raises its interrupt through the controller each tick. The kernel drains the pending set and dispatches the tick ISR, which does three things: it releases periodic and one-shot jobs whose release time has arrived, it wakes tasks whose delay has expired, and it flags any active job that has reached its deadline without completing. This is the software side of an interrupt service routine reacting to a hardware timer, and it is where releases and wakeups that signal tasks originate. Other IRQ lines (UART, GPIO) are modeled by the controller and available for extension.

## Why each gate proves its claim

The gates are in `tests/` and run in CI. `EMBERCHIP_FUZZ_OPS` widens the randomized runs.

### Gate 1, fixed-priority preemptive correctness

`tests/preemption.rs`. Each tick the kernel records the ready set with each task's priority and which task actually ran. The test replays those records and asserts, independently of the scheduler, that the task that ran had the highest priority of every task that was ready at that instant. These random sets use no mutexes, so effective priority equals base priority and the check is genuine rather than circular. Separately, because each set is generated at or below the rate-monotonic bound with rate-monotonic priorities, the test asserts zero missed deadlines over a bounded horizon. If the scheduler ever let a lower-priority task run while a higher-priority one was ready, or dropped a deadline on a schedulable set, the gate fails.

A further case adds real mutex contention. Many tasks, including blocks of deliberately equal priority, share one mutex around a phased low holder, so inheritance is constantly boosting and restoring. The test checks from the recorded snapshots alone that the running task always carries the maximum effective priority (not just base priority), replays lock and unlock events to confirm the mutex is never doubly held, and asserts that contention actually happened so the check is not vacuous.

### Gate 2, priority inheritance bounds inversion

`tests/priority_inheritance.rs`. The scenario has a low task holding a mutex across a bounded critical section, a medium pure-compute task, and a high task that needs the mutex. The test measures the number of ticks the high task is blocked between first requesting the mutex and acquiring it. With inheritance on, that blocking is bounded by the critical section (plus a small scheduling slack) because the boosted low holder cannot be preempted by the medium task. With inheritance off, the medium task preempts the holder and the blocking stretches well past the critical section. The test asserts both the bound and the strict contrast, so the effect of inheritance is proven from behavior, not asserted. It also checks that a boost is logged and then restored, and that no boost happens when the feature is off.

A nested case extends this to a chain. A low task holds one mutex, a medium task holds a second mutex and blocks on the first, and a high task blocks on the second. With inheritance the boost from the high task must reach the medium holder and, transitively, the low holder two levels down. The test asserts both holders are boosted to the high priority and then restored, that the high task's blocking stays inside the two nested critical sections, and that a no-inheritance run is strictly worse.

### Gate 3, synchronization correctness and determinism

`tests/synchronization.rs`. Mutual exclusion is checked by replaying the lock and unlock events and asserting the protected section is never held by two tasks at once. Semaphore conservation is checked by signaling a fixed number of times, waiting the same number, and asserting no unit is lost or invented and the consumer finishes. Queue integrity is checked by sending a known sequence and asserting the received sequence is identical in order and content. Determinism is checked by running the same seed and task set twice and asserting the two event logs are byte-identical, and separately that the demo produces the same timeline and UART output across runs.

### Gate 4, schedulability analysis matches the simulator

`tests/schedulability.rs`. A generator produces random rate-monotonic task sets whose load ranges from light to overloaded, so both schedulable and unschedulable sets appear. For each, exact response-time analysis returns a verdict and the simulator runs the set for a hyperperiod. Because the simulator releases every task at the critical instant, the analysis is exact, so the test asserts the two agree in both directions: a set the analysis calls schedulable misses no deadline, and a set it calls unschedulable misses at least one. A second test asserts the utilization bound is conservative, meaning every set within the bound also passes exact analysis. The test requires both outcomes to appear, so neither branch of the correspondence can pass vacuously. This gate is what surfaced an off-by-one in the deadline check, where a job whose worst-case response time equals its period was allowed one extra tick and its miss went uncounted.

Every module also carries focused unit tests: scheduler pick, mutex grant order, semaphore conservation and saturation, queue FIFO and fullness, timer wrap, interrupt dispatch order, and response-time analysis.

## The stress harness

`src/bin/stress.rs` is a separate, max-scale adversarial harness, run in release and not part of `cargo test`. It drives the kernel for a configured number of ticks while folding the event log in chunks and clearing it, so memory stays flat regardless of horizon. Six scenarios each target one gate under load: a lock storm that asserts the inheritance blocking bound while confirming the boss really blocked and that a no-inheritance run is strictly worse, a semaphore thundering herd checked for exact conservation, queue churn checked for FIFO order and conservation, a hundreds-of-tasks scenario replayed against the effective-priority scheduling invariant every tick, a determinism hash of the whole event stream across two runs, and a two-mutex circular wait that must be contained without corrupting the rest of the system. Every scenario is sized from demand so it stays schedulable at any scale, and each asserts that its traffic actually flowed, so it cannot pass empty.
