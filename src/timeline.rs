//! Human-readable rendering of the event log. The scheduler timeline, GPIO and
//! UART activity, and every mutex and inheritance event are printed from the
//! same log the determinism gate compares, so what you read is exactly what the
//! kernel did.

use crate::kernel::{Event, Kernel};

/// Render the full event log as an indented, tick-by-tick timeline.
pub fn render(kernel: &Kernel) -> String {
    let mut out = String::new();
    let name = |id: usize| kernel.tasks.get(id).map(|t| t.name.as_str()).unwrap_or("?");
    for ev in &kernel.log {
        match ev {
            Event::Tick(t) => {
                out.push_str(&format!("\ntick {t:>4} |"));
            }
            Event::Run { task } => {
                out.push_str(&format!("\n         | RUN {}#{task}", name(*task)));
            }
            Event::Idle => out.push_str("\n         | IDLE"),
            Event::Release { task } => {
                out.push_str(&format!("\n         | release {}#{task}", name(*task)));
            }
            Event::Preempt { preempted, by } => {
                out.push_str(&format!(
                    "\n         | PREEMPT {}#{preempted} by {}#{by}",
                    name(*preempted),
                    name(*by)
                ));
            }
            Event::Gpio { task, pin, level } => {
                let s = if *level { "HIGH" } else { "LOW" };
                let led = if *pin == crate::mcu::LED_PIN {
                    if *level {
                        " (LED on)"
                    } else {
                        " (LED off)"
                    }
                } else {
                    ""
                };
                out.push_str(&format!("\n         | gpio[{pin}]={s}{led} by {}", name(*task)));
            }
            Event::Uart { task, text } => {
                let shown = text.replace('\n', "\\n");
                out.push_str(&format!("\n         | uart<= \"{shown}\" by {}", name(*task)));
            }
            Event::Lock { task, mutex } => {
                out.push_str(&format!("\n         | lock m{mutex} by {}#{task}", name(*task)));
            }
            Event::Unlock { task, mutex } => {
                out.push_str(&format!("\n         | unlock m{mutex} by {}#{task}", name(*task)));
            }
            Event::BlockOnMutex { task, mutex, owner } => {
                out.push_str(&format!(
                    "\n         | BLOCK {}#{task} on m{mutex} held by {}#{owner}",
                    name(*task),
                    name(*owner)
                ));
            }
            Event::Inherit {
                holder,
                boosted_to,
                waiter,
            } => {
                out.push_str(&format!(
                    "\n         | INHERIT {}#{holder} boosted to prio {boosted_to} (waiter {}#{waiter})",
                    name(*holder),
                    name(*waiter)
                ));
            }
            Event::Restore { holder, to } => {
                out.push_str(&format!(
                    "\n         | restore {}#{holder} to prio {to}",
                    name(*holder)
                ));
            }
            Event::SemWait { task, sem } => {
                out.push_str(&format!("\n         | sem{sem} wait by {}#{task}", name(*task)));
            }
            Event::SemSignal { task, sem } => {
                out.push_str(&format!("\n         | sem{sem} signal by {}#{task}", name(*task)));
            }
            Event::SemBlock { task, sem } => {
                out.push_str(&format!("\n         | BLOCK {}#{task} on sem{sem}", name(*task)));
            }
            Event::QueueSend { task, queue, value } => {
                out.push_str(&format!(
                    "\n         | q{queue} send {value} by {}#{task}",
                    name(*task)
                ));
            }
            Event::QueueRecv { task, queue, value } => {
                out.push_str(&format!(
                    "\n         | q{queue} recv {value} by {}#{task}",
                    name(*task)
                ));
            }
            Event::QueueBlock { task, queue } => {
                out.push_str(&format!("\n         | BLOCK {}#{task} on q{queue}", name(*task)));
            }
            Event::Sleep { task, until } => {
                out.push_str(&format!(
                    "\n         | sleep {}#{task} until {until}",
                    name(*task)
                ));
            }
            Event::Complete { task } => {
                out.push_str(&format!("\n         | done {}#{task}", name(*task)));
            }
            Event::DeadlineMiss { task, deadline } => {
                out.push_str(&format!(
                    "\n         | DEADLINE MISS {}#{task} (was due {deadline})",
                    name(*task)
                ));
            }
        }
    }
    out.push('\n');
    out
}

/// A compact per-task summary printed after a run.
pub fn summary(kernel: &Kernel) -> String {
    let mut out = String::new();
    out.push_str("\ntask summary\n");
    out.push_str("  id  name        prio  period  wcet  runs  done  missed  cpu\n");
    for t in &kernel.tasks {
        let period = t
            .period
            .map(|p| p.to_string())
            .unwrap_or_else(|| "once".to_string());
        out.push_str(&format!(
            "  {:>2}  {:<10}  {:>4}  {:>6}  {:>4}  {:>4}  {:>4}  {:>6}  {:>4}\n",
            t.id,
            t.name,
            t.base_priority,
            period,
            t.wcet,
            t.jobs_released,
            t.jobs_completed,
            t.deadlines_missed,
            t.cpu_ticks,
        ));
    }
    out.push_str(&format!(
        "\nticks {}  idle {}  timer irqs {}  uart bytes {}  invariant violations {}  deadline misses {}\n",
        kernel.now(),
        kernel.idle_ticks(),
        kernel.mcu.timer.interrupts_raised(),
        kernel.mcu.uart.bytes_out(),
        kernel.invariant_violations(),
        kernel.total_deadline_misses(),
    ));
    out
}
