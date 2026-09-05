//! Emberchip: a deterministic RTOS simulator on a small emulated microcontroller.
//!
//! This is a teaching-accurate model, not firmware for a real chip. It runs in
//! pure Rust std on the host and simulates the mechanisms a real-time operating
//! system is built from: a preemptive fixed-priority scheduler driven by a tick
//! timer, interrupts and ISRs, mutexes with priority inheritance, counting
//! semaphores, message queues, and memory-mapped peripherals (GPIO, UART, a
//! timer). Given the same seed and task set the entire run reproduces exactly,
//! down to the event timeline, which is what makes the correctness gates
//! checkable.
//!
//! See `DESIGN.md` for the model and `README.md` for the honest framing.

pub mod kernel;
pub mod mcu;
pub mod rng;
pub mod timeline;
pub mod workload;

pub use kernel::{
    BlockReason, Config, Event, Kernel, Mutex, Op, Priority, Queue, ReadyEntry, Semaphore, Task,
    TaskState, TickRecord,
};
pub use mcu::Mcu;
pub use rng::Rng;

/// Read the fuzzing operation budget from `EMBERCHIP_FUZZ_OPS`, falling back to
/// `default` when it is unset or unparseable. Used to keep the randomized gates
/// bounded in CI while letting a deeper local run be requested.
pub fn fuzz_ops(default: u64) -> u64 {
    std::env::var("EMBERCHIP_FUZZ_OPS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}
