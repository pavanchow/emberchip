//! The RTOS kernel: task table, synchronization objects, the preemptive
//! fixed-priority scheduler, and the tick-driven run loop that ties them to the
//! emulated hardware.
//!
//! Time is discrete. Each call to [`Kernel::step`] is one system tick: the timer
//! raises its interrupt, the tick ISR releases due jobs and wakes sleepers, the
//! scheduler picks the highest-priority ready task, and that task runs for one
//! quantum. The whole run is a pure function of the seed and the task set, so it
//! reproduces exactly.

pub mod event;
pub mod mutex;
pub mod queue;
pub mod semaphore;
pub mod task;

pub use event::Event;
pub use mutex::Mutex;
pub use queue::Queue;
pub use semaphore::Semaphore;
pub use task::{compute_budget, BlockReason, Op, Priority, Task, TaskState};

use crate::mcu::{Irq, Mcu, DEFAULT_RAM};
use crate::rng::Rng;

/// Kernel configuration.
#[derive(Clone, Debug)]
pub struct Config {
    pub ram: usize,
    pub tick_reload: u64,
    /// When false, mutexes give mutual exclusion but do not boost the holder,
    /// which is how the unbounded-inversion contrast is produced.
    pub priority_inheritance: bool,
    pub seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ram: DEFAULT_RAM,
            tick_reload: 1,
            priority_inheritance: true,
            seed: 1,
        }
    }
}

/// One snapshot of a ready task at a scheduling decision.
#[derive(Clone, Copy, Debug)]
pub struct ReadyEntry {
    pub id: usize,
    pub base: Priority,
    pub eff: Priority,
}

/// What the scheduler saw and decided on a single tick. Kept so tests can check
/// the scheduling invariant independently of the scheduler's own logic.
#[derive(Clone, Debug)]
pub struct TickRecord {
    pub tick: u64,
    pub ran: Option<usize>,
    pub ready: Vec<ReadyEntry>,
}

pub struct Kernel {
    pub mcu: Mcu,
    pub tasks: Vec<Task>,
    pub mutexes: Vec<Mutex>,
    pub semaphores: Vec<Semaphore>,
    pub queues: Vec<Queue>,
    pub log: Vec<Event>,
    pub records: Vec<TickRecord>,
    pub rng: Rng,
    pub config: Config,
    now: u64,
    running: Option<usize>,
    idle_ticks: u64,
    invariant_violations: u64,
}

impl Kernel {
    pub fn new(config: Config) -> Self {
        let mcu = Mcu::new(config.ram, config.tick_reload);
        let rng = Rng::new(config.seed);
        Self {
            mcu,
            tasks: Vec::new(),
            mutexes: Vec::new(),
            semaphores: Vec::new(),
            queues: Vec::new(),
            log: Vec::new(),
            records: Vec::new(),
            rng,
            config,
            now: 0,
            running: None,
            idle_ticks: 0,
            invariant_violations: 0,
        }
    }

    // ----- construction -----

    pub fn add_task(&mut self, mut task: Task) -> usize {
        let id = self.tasks.len();
        task.id = id;
        // Periodic and one-shot tasks are both first released at tick 1.
        task.next_release = 1;
        task.state = TaskState::Suspended;
        self.tasks.push(task);
        id
    }

    pub fn add_mutex(&mut self) -> usize {
        self.mutexes.push(Mutex::new());
        self.mutexes.len() - 1
    }

    pub fn add_semaphore(&mut self, initial: u32, max: u32) -> usize {
        self.semaphores.push(Semaphore::new(initial, max));
        self.semaphores.len() - 1
    }

    pub fn add_queue(&mut self, capacity: usize) -> usize {
        self.queues.push(Queue::new(capacity));
        self.queues.len() - 1
    }

    // ----- accessors -----

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn idle_ticks(&self) -> u64 {
        self.idle_ticks
    }

    pub fn invariant_violations(&self) -> u64 {
        self.invariant_violations
    }

    pub fn running(&self) -> Option<usize> {
        self.running
    }

    pub fn total_deadline_misses(&self) -> u64 {
        self.tasks.iter().map(|t| t.deadlines_missed).sum()
    }

    fn eff_snapshot(&self) -> Vec<Priority> {
        self.tasks.iter().map(|t| t.eff_priority).collect()
    }

    // ----- the run loop -----

    pub fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.step();
        }
    }

    pub fn step(&mut self) {
        self.now += 1;
        self.log.push(Event::Tick(self.now));

        self.mcu.cycle();
        for irq in self.mcu.nvic.take_pending() {
            if irq == Irq::Timer {
                self.on_tick();
            }
        }

        let ready: Vec<ReadyEntry> = self
            .tasks
            .iter()
            .filter(|t| matches!(t.state, TaskState::Ready | TaskState::Running))
            .map(|t| ReadyEntry {
                id: t.id,
                base: t.base_priority,
                eff: t.eff_priority,
            })
            .collect();

        let chosen = self.pick();
        self.records.push(TickRecord {
            tick: self.now,
            ran: chosen,
            ready: ready.clone(),
        });

        match chosen {
            None => {
                self.running = None;
                self.idle_ticks += 1;
                self.log.push(Event::Idle);
            }
            Some(tid) => {
                let max_eff = ready.iter().map(|r| r.eff).max().unwrap_or(0);
                if self.tasks[tid].eff_priority < max_eff {
                    self.invariant_violations += 1;
                }
                if let Some(prev) = self.running {
                    if prev != tid && self.tasks[prev].state == TaskState::Running {
                        self.tasks[prev].state = TaskState::Ready;
                        self.tasks[prev].preemptions += 1;
                        self.log.push(Event::Preempt {
                            preempted: prev,
                            by: tid,
                        });
                    }
                }
                self.running = Some(tid);
                self.tasks[tid].state = TaskState::Running;
                self.log.push(Event::Run { task: tid });
                self.run_quantum(tid);
            }
        }
    }

    /// The tick ISR: release due jobs, wake sleepers, flag missed deadlines.
    fn on_tick(&mut self) {
        let now = self.now;
        for i in 0..self.tasks.len() {
            let t = &self.tasks[i];
            if !t.job_active && t.next_release != u64::MAX && now >= t.next_release {
                let period = t.period;
                let t = &mut self.tasks[i];
                t.state = TaskState::Ready;
                t.job_active = true;
                t.jobs_released += 1;
                t.pc = 0;
                t.compute_remaining = 0;
                t.blocked_on = None;
                t.missed_current = false;
                match period {
                    Some(p) => {
                        t.deadline = now + p;
                        t.next_release += p;
                    }
                    None => {
                        t.deadline = u64::MAX;
                        t.next_release = u64::MAX;
                    }
                }
                self.log.push(Event::Release { task: i });
            }
        }

        for i in 0..self.tasks.len() {
            let t = &self.tasks[i];
            if t.state == TaskState::Blocked
                && t.blocked_on == Some(BlockReason::Sleep)
                && now >= t.wake_at
            {
                let t = &mut self.tasks[i];
                t.state = TaskState::Ready;
                t.blocked_on = None;
            }
        }

        for i in 0..self.tasks.len() {
            let t = &self.tasks[i];
            if t.job_active && !t.missed_current && t.deadline != u64::MAX && now > t.deadline {
                let deadline = t.deadline;
                let t = &mut self.tasks[i];
                t.deadlines_missed += 1;
                t.missed_current = true;
                self.log.push(Event::DeadlineMiss { task: i, deadline });
            }
        }
    }

    /// Highest effective priority among ready and running tasks, ties broken by
    /// lowest task id for determinism.
    fn pick(&self) -> Option<usize> {
        let mut best: Option<usize> = None;
        for t in &self.tasks {
            if !matches!(t.state, TaskState::Ready | TaskState::Running) {
                continue;
            }
            match best {
                None => best = Some(t.id),
                Some(b) => {
                    let bp = self.tasks[b].eff_priority;
                    if t.eff_priority > bp || (t.eff_priority == bp && t.id < b) {
                        best = Some(t.id);
                    }
                }
            }
        }
        best
    }

    /// Run one task for a single quantum. Instantaneous operations (peripheral
    /// writes, non-blocking synchronization) chain within the quantum until a
    /// Compute step consumes the CPU tick, the task blocks, or the job ends.
    fn run_quantum(&mut self, tid: usize) {
        loop {
            let op = self.tasks[tid].program.get(self.tasks[tid].pc).cloned();
            let op = match op {
                Some(op) => op,
                None => {
                    self.complete_job(tid);
                    return;
                }
            };

            match op {
                Op::Compute(n) => {
                    if self.tasks[tid].compute_remaining == 0 {
                        self.tasks[tid].compute_remaining = n;
                    }
                    if self.tasks[tid].compute_remaining == 0 {
                        self.tasks[tid].pc += 1;
                        continue;
                    }
                    self.tasks[tid].compute_remaining -= 1;
                    self.tasks[tid].cpu_ticks += 1;
                    if self.tasks[tid].compute_remaining == 0 {
                        self.tasks[tid].pc += 1;
                        // Recognize end-of-program on the same tick the final
                        // compute completes, so a job costs exactly its WCET in
                        // CPU ticks and never an extra idle tick to notice it is
                        // done. This keeps the schedulability model exact.
                        if self.tasks[tid].pc >= self.tasks[tid].program.len() {
                            self.complete_job(tid);
                        }
                    }
                    return;
                }
                Op::Delay(n) => {
                    self.tasks[tid].pc += 1;
                    if n == 0 {
                        continue;
                    }
                    let until = self.now + n;
                    let t = &mut self.tasks[tid];
                    t.state = TaskState::Blocked;
                    t.blocked_on = Some(BlockReason::Sleep);
                    t.wake_at = until;
                    self.running = None;
                    self.log.push(Event::Sleep { task: tid, until });
                    return;
                }
                Op::GpioSet(pin, level) => {
                    self.mcu.gpio.set(pin, level);
                    self.tasks[tid].pc += 1;
                    self.log.push(Event::Gpio {
                        task: tid,
                        pin,
                        level,
                    });
                }
                Op::GpioToggle(pin) => {
                    let level = self.mcu.gpio.toggle(pin).unwrap_or(false);
                    self.tasks[tid].pc += 1;
                    self.log.push(Event::Gpio {
                        task: tid,
                        pin,
                        level,
                    });
                }
                Op::Uart(text) => {
                    self.mcu.uart.write(&text);
                    self.tasks[tid].pc += 1;
                    self.log.push(Event::Uart { task: tid, text });
                }
                Op::Lock(m) => {
                    if self.mutexes[m].try_acquire(tid) {
                        self.tasks[tid].held_mutexes.push(m);
                        self.tasks[tid].pc += 1;
                        self.log.push(Event::Lock { task: tid, mutex: m });
                    } else {
                        let owner = self.mutexes[m].owner.unwrap();
                        self.mutexes[m].add_waiter(tid);
                        let t = &mut self.tasks[tid];
                        t.state = TaskState::Blocked;
                        t.blocked_on = Some(BlockReason::Mutex(m));
                        self.running = None;
                        self.log.push(Event::BlockOnMutex {
                            task: tid,
                            mutex: m,
                            owner,
                        });
                        if self.config.priority_inheritance {
                            self.apply_inheritance(tid);
                        }
                        return;
                    }
                }
                Op::Unlock(m) => {
                    let snap = self.eff_snapshot();
                    let next = self.mutexes[m].release(tid, |t| snap[t]);
                    self.tasks[tid].held_mutexes.retain(|&x| x != m);
                    self.tasks[tid].pc += 1;
                    self.log.push(Event::Unlock { task: tid, mutex: m });
                    if self.config.priority_inheritance {
                        self.recompute_holder(tid);
                    }
                    if let Some(n) = next {
                        // The waiter is handed the mutex directly. It was parked
                        // on its Lock op, so advance past it and record the
                        // acquisition, otherwise it would re-run Lock, see itself
                        // as owner, and block forever.
                        self.tasks[n].held_mutexes.push(m);
                        self.tasks[n].state = TaskState::Ready;
                        self.tasks[n].blocked_on = None;
                        self.tasks[n].pc += 1;
                        self.log.push(Event::Lock { task: n, mutex: m });
                        if self.config.priority_inheritance {
                            self.recompute_holder(n);
                        }
                    }
                }
                Op::SemWait(s) => {
                    if self.semaphores[s].try_wait() {
                        self.tasks[tid].pc += 1;
                        self.log.push(Event::SemWait { task: tid, sem: s });
                    } else {
                        self.semaphores[s].add_waiter(tid);
                        let t = &mut self.tasks[tid];
                        t.state = TaskState::Blocked;
                        t.blocked_on = Some(BlockReason::Semaphore(s));
                        self.running = None;
                        self.log.push(Event::SemBlock { task: tid, sem: s });
                        return;
                    }
                }
                Op::SemSignal(s) => {
                    let snap = self.eff_snapshot();
                    let woken = self.semaphores[s].signal(|t| snap[t]);
                    self.tasks[tid].pc += 1;
                    self.log.push(Event::SemSignal { task: tid, sem: s });
                    if let Some(w) = woken {
                        // The unit was handed to this waiter, not returned to the
                        // count, so advance it past its SemWait op and record the
                        // take. Otherwise it would re-run SemWait against a zero
                        // count and block again.
                        self.tasks[w].state = TaskState::Ready;
                        self.tasks[w].blocked_on = None;
                        self.tasks[w].pc += 1;
                        self.log.push(Event::SemWait { task: w, sem: s });
                    }
                }
                Op::QueueSend(q, value) => {
                    if self.queues[q].try_send(value) {
                        self.tasks[tid].pc += 1;
                        self.log.push(Event::QueueSend {
                            task: tid,
                            queue: q,
                            value,
                        });
                        let snap = self.eff_snapshot();
                        if let Some(r) = self.queues[q].wake_receiver(|t| snap[t]) {
                            self.tasks[r].state = TaskState::Ready;
                            self.tasks[r].blocked_on = None;
                        }
                    } else {
                        self.queues[q].send_waiters.push(tid);
                        let t = &mut self.tasks[tid];
                        t.state = TaskState::Blocked;
                        t.blocked_on = Some(BlockReason::QueueSend(q));
                        self.running = None;
                        self.log.push(Event::QueueBlock {
                            task: tid,
                            queue: q,
                        });
                        return;
                    }
                }
                Op::QueueRecv(q) => match self.queues[q].try_recv() {
                    Some(value) => {
                        self.tasks[tid].pc += 1;
                        self.log.push(Event::QueueRecv {
                            task: tid,
                            queue: q,
                            value,
                        });
                        let snap = self.eff_snapshot();
                        if let Some(s) = self.queues[q].wake_sender(|t| snap[t]) {
                            self.tasks[s].state = TaskState::Ready;
                            self.tasks[s].blocked_on = None;
                        }
                    }
                    None => {
                        self.queues[q].recv_waiters.push(tid);
                        let t = &mut self.tasks[tid];
                        t.state = TaskState::Blocked;
                        t.blocked_on = Some(BlockReason::QueueRecv(q));
                        self.running = None;
                        self.log.push(Event::QueueBlock {
                            task: tid,
                            queue: q,
                        });
                        return;
                    }
                },
            }
        }
    }

    fn complete_job(&mut self, tid: usize) {
        let periodic = self.tasks[tid].period.is_some();
        let t = &mut self.tasks[tid];
        t.jobs_completed += 1;
        t.job_active = false;
        t.pc = 0;
        t.compute_remaining = 0;
        if periodic {
            t.state = TaskState::Blocked;
            t.blocked_on = Some(BlockReason::WaitPeriod);
        } else {
            t.state = TaskState::Suspended;
            t.next_release = u64::MAX;
        }
        self.running = None;
        self.log.push(Event::Complete { task: tid });
    }

    /// Walk the chain of blocked owners, boosting each holder to the priority of
    /// the task waiting on it. This is priority inheritance, including the nested
    /// case where a holder is itself blocked on another mutex.
    fn apply_inheritance(&mut self, blocked_task: usize) {
        let mut current = blocked_task;
        while let Some(BlockReason::Mutex(mutex_id)) = self.tasks[current].blocked_on {
            let owner = match self.mutexes[mutex_id].owner {
                Some(o) => o,
                None => break,
            };
            let want = self.tasks[current].eff_priority;
            if want > self.tasks[owner].eff_priority {
                self.tasks[owner].eff_priority = want;
                self.log.push(Event::Inherit {
                    holder: owner,
                    boosted_to: want,
                    waiter: current,
                });
                current = owner;
            } else {
                break;
            }
        }
    }

    /// Recompute a holder's effective priority from its base and the tasks still
    /// waiting on the mutexes it holds. Used after a release to drop a boost that
    /// is no longer justified.
    fn recompute_holder(&mut self, holder: usize) {
        let held = self.tasks[holder].held_mutexes.clone();
        let mut p = self.tasks[holder].base_priority;
        for m in held {
            for &w in &self.mutexes[m].waiters {
                p = p.max(self.tasks[w].eff_priority);
            }
        }
        if p != self.tasks[holder].eff_priority {
            self.tasks[holder].eff_priority = p;
            self.log.push(Event::Restore { holder, to: p });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic() -> Config {
        Config {
            seed: 5,
            ..Config::default()
        }
    }

    #[test]
    fn scheduler_pick_prefers_higher_priority() {
        let mut k = Kernel::new(basic());
        k.add_task(Task::new(0, "lo", 1).periodic(100, vec![Op::Compute(50)]));
        k.add_task(Task::new(1, "hi", 9).periodic(100, vec![Op::Compute(50)]));
        k.step();
        assert_eq!(k.running(), Some(1));
    }

    #[test]
    fn higher_priority_preempts_lower() {
        // Low runs first, high is released later and must preempt.
        let mut k = Kernel::new(basic());
        k.add_task(Task::new(0, "lo", 1).periodic(100, vec![Op::Compute(50)]));
        let mut hi = Task::new(1, "hi", 9).periodic(100, vec![Op::Compute(5)]);
        hi.next_release = 0; // placeholder, overwritten by add_task
        k.add_task(hi);
        // Force high to be released a few ticks in by giving it a later phase.
        k.tasks[1].next_release = 4;
        k.run(6);
        let preempted = k
            .log
            .iter()
            .any(|e| matches!(e, Event::Preempt { preempted: 0, by: 1 }));
        assert!(preempted, "high priority task should preempt low");
    }

    #[test]
    fn mutex_gives_mutual_exclusion() {
        let mut k = Kernel::new(basic());
        let m = k.add_mutex();
        k.add_task(Task::new(0, "a", 5).periodic(
            100,
            vec![Op::Lock(m), Op::Compute(3), Op::Unlock(m)],
        ));
        k.add_task(Task::new(1, "b", 6).periodic(
            100,
            vec![Op::Lock(m), Op::Compute(3), Op::Unlock(m)],
        ));
        k.run(30);
        // The mutex ends free and no invariant was violated.
        assert!(k.mutexes[m].is_free());
        assert_eq!(k.invariant_violations(), 0);
    }
}
