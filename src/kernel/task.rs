//! Tasks and their execution model.
//!
//! A task is described by a program: a list of operations it performs each job.
//! Compute steps burn CPU ticks, the rest touch peripherals or synchronization
//! primitives. The scheduler runs the program forward one quantum at a time,
//! which is how a real task makes progress between preemptions.

/// Priority. A larger number is more urgent. The scheduler always runs the
/// ready task with the highest effective priority.
pub type Priority = u8;

/// One instruction in a task program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Burn `n` ticks of CPU. This is the work the schedulability model counts.
    Compute(u64),
    /// Sleep for `n` ticks, then become ready again.
    Delay(u64),
    /// Drive a GPIO pin to a level.
    GpioSet(usize, bool),
    /// Flip a GPIO pin.
    GpioToggle(usize),
    /// Emit text on the UART.
    Uart(String),
    /// Acquire a mutex, blocking (with priority inheritance) if it is held.
    Lock(usize),
    /// Release a mutex.
    Unlock(usize),
    /// Take one unit from a counting semaphore, blocking if it is zero.
    SemWait(usize),
    /// Give one unit to a counting semaphore, waking a waiter if any.
    SemSignal(usize),
    /// Send a value into a message queue, blocking if it is full.
    QueueSend(usize, u32),
    /// Receive a value from a message queue, blocking if it is empty.
    QueueRecv(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Suspended,
}

/// Why a blocked task is waiting. Used only for reporting and wakeups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    Sleep,
    Mutex(usize),
    Semaphore(usize),
    QueueRecv(usize),
    QueueSend(usize),
    WaitPeriod,
}

#[derive(Clone, Debug)]
pub struct Task {
    pub id: usize,
    pub name: String,
    pub base_priority: Priority,
    /// Current priority, possibly boosted by inheritance while holding a mutex.
    pub eff_priority: Priority,
    /// Period in ticks for a periodic task. `None` for a one-shot task.
    pub period: Option<u64>,
    /// Worst-case execution time in ticks, for the schedulability model.
    pub wcet: u64,

    pub state: TaskState,
    pub program: Vec<Op>,
    pub pc: usize,
    pub compute_remaining: u64,

    pub next_release: u64,
    pub wake_at: u64,
    pub deadline: u64,
    pub job_active: bool,
    pub missed_current: bool,

    /// The task stack, held as plain memory. Reserved region, not addressed by
    /// the simple program model, but sized and present as real stacks are.
    pub stack: Vec<u8>,
    pub held_mutexes: Vec<usize>,
    pub blocked_on: Option<BlockReason>,

    pub jobs_released: u64,
    pub jobs_completed: u64,
    pub deadlines_missed: u64,
    pub cpu_ticks: u64,
    pub preemptions: u64,
}

impl Task {
    pub fn new(id: usize, name: impl Into<String>, priority: Priority) -> Self {
        Self {
            id,
            name: name.into(),
            base_priority: priority,
            eff_priority: priority,
            period: None,
            wcet: 0,
            state: TaskState::Suspended,
            program: Vec::new(),
            pc: 0,
            compute_remaining: 0,
            next_release: 0,
            wake_at: 0,
            deadline: 0,
            job_active: false,
            missed_current: false,
            stack: vec![0; 512],
            held_mutexes: Vec::new(),
            blocked_on: None,
            jobs_released: 0,
            jobs_completed: 0,
            deadlines_missed: 0,
            cpu_ticks: 0,
            preemptions: 0,
        }
    }

    /// A periodic task: released every `period` ticks with an implicit deadline
    /// equal to the period.
    #[must_use]
    pub fn periodic(mut self, period: u64, program: Vec<Op>) -> Self {
        self.period = Some(period);
        self.wcet = compute_budget(&program);
        self.program = program;
        self
    }

    /// A one-shot task, released once at startup.
    #[must_use]
    pub fn oneshot(mut self, program: Vec<Op>) -> Self {
        self.period = None;
        self.wcet = compute_budget(&program);
        self.program = program;
        self
    }

    /// With a fixed-size stack, in bytes.
    #[must_use]
    pub fn with_stack(mut self, bytes: usize) -> Self {
        self.stack = vec![0; bytes];
        self
    }

    pub fn utilization(&self) -> f64 {
        match self.period {
            Some(p) if p > 0 => self.wcet as f64 / p as f64,
            _ => 0.0,
        }
    }
}

/// Total CPU ticks a single job of this program requires.
pub fn compute_budget(program: &[Op]) -> u64 {
    program
        .iter()
        .map(|op| match op {
            Op::Compute(n) => *n,
            _ => 0,
        })
        .sum()
}
