//! The event log. Every meaningful thing the kernel does in a tick becomes an
//! `Event`. The log is the timeline the CLI prints, and because it is a plain
//! value with `Eq`, two runs with the same seed and task set can be compared
//! for exact equality, which is how the determinism gate is checked.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    /// A new tick began at this time.
    Tick(u64),
    /// A periodic (or one-shot) job of a task was released.
    Release { task: usize },
    /// The task that ran on the CPU this tick.
    Run { task: usize },
    /// No task was ready, the CPU idled.
    Idle,
    /// A running task was preempted by a higher-priority task.
    Preempt { preempted: usize, by: usize },
    /// A GPIO pin was driven.
    Gpio { task: usize, pin: usize, level: bool },
    /// Text was written to the UART.
    Uart { task: usize, text: String },
    /// A mutex was acquired.
    Lock { task: usize, mutex: usize },
    /// A mutex was released.
    Unlock { task: usize, mutex: usize },
    /// A task blocked waiting for a mutex held by another task.
    BlockOnMutex { task: usize, mutex: usize, owner: usize },
    /// The mutex holder had its priority boosted by inheritance.
    Inherit { holder: usize, boosted_to: u8, waiter: usize },
    /// The mutex holder's priority was restored after release.
    Restore { holder: usize, to: u8 },
    /// A semaphore unit was taken.
    SemWait { task: usize, sem: usize },
    /// A semaphore was signaled.
    SemSignal { task: usize, sem: usize },
    /// A task blocked on an empty semaphore.
    SemBlock { task: usize, sem: usize },
    /// A value was sent into a queue.
    QueueSend { task: usize, queue: usize, value: u32 },
    /// A value was received from a queue.
    QueueRecv { task: usize, queue: usize, value: u32 },
    /// A task blocked on a queue (full for send, empty for receive).
    QueueBlock { task: usize, queue: usize },
    /// A task went to sleep until the given tick.
    Sleep { task: usize, until: u64 },
    /// A job finished its program.
    Complete { task: usize },
    /// A job missed its deadline.
    DeadlineMiss { task: usize, deadline: u64 },
}
