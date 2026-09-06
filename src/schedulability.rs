//! Rate-monotonic schedulability analysis.
//!
//! Two classic tests for preemptive fixed-priority scheduling of independent
//! periodic tasks, and the machinery to check their predictions against the
//! simulator:
//!
//!  - The Liu and Layland utilization bound: if total utilization is at or
//!    below `n * (2^(1/n) - 1)` the set is schedulable. This is sufficient but
//!    not necessary, so a set can fail it and still meet every deadline.
//!  - Exact response-time analysis (RTA): iterate each task's worst-case
//!    response time to a fixed point and compare it to the deadline. For a
//!    synchronous, independent task set (every task released together at the
//!    critical instant, which is exactly what the simulator does) this is both
//!    necessary and sufficient, so its verdict must match the simulated run
//!    tick for tick.

use crate::kernel::Task;
use crate::workload::rm_bound;

/// The result of analyzing a task set.
#[derive(Clone, Debug)]
pub struct Analysis {
    pub utilization: f64,
    pub utilization_bound: f64,
    /// True if total utilization is within the Liu and Layland bound. Sufficient
    /// but not necessary for schedulability.
    pub utilization_ok: bool,
    /// Worst-case response time per task, in input order. `None` means the
    /// response time grew past the deadline, so that task is unschedulable.
    pub response_times: Vec<Option<u64>>,
    /// True when every task's response time is within its deadline. Exact for a
    /// synchronous, independent, preemptive fixed-priority task set.
    pub schedulable: bool,
}

/// Exact worst-case response time of a task with the given WCET and (implicit)
/// deadline, interfered with by the higher-priority tasks in `hp`, each listed
/// as `(period, wcet)`. Returns `None` if the recurrence exceeds the deadline.
///
/// The recurrence is `R = C + sum_j ceil(R / T_j) * C_j`, iterated from `R = C`
/// upward. It is monotonic and converges, or crosses the deadline first.
fn response_time(wcet: u64, deadline: u64, hp: &[(u64, u64)]) -> Option<u64> {
    let mut r = wcet;
    loop {
        let interference: u64 = hp.iter().map(|&(t, c)| r.div_ceil(t) * c).sum();
        let next = wcet + interference;
        if next > deadline {
            return None;
        }
        if next == r {
            return Some(r);
        }
        r = next;
    }
}

/// Analyze a periodic task set. Deadlines are implicit (deadline == period) and
/// priorities are taken from each task's base priority (a larger number is more
/// urgent). Tasks with no period are ignored.
pub fn analyze(tasks: &[Task]) -> Analysis {
    let periodic: Vec<&Task> = tasks.iter().filter(|t| t.period.is_some()).collect();
    let n = periodic.len();

    let utilization: f64 = periodic.iter().map(|t| t.utilization()).sum();
    let utilization_bound = rm_bound(n.max(1));
    let utilization_ok = utilization <= utilization_bound + 1e-9;

    let mut response_times = Vec::with_capacity(n);
    let mut schedulable = true;
    for t in &periodic {
        let period = t.period.unwrap();
        let hp: Vec<(u64, u64)> = periodic
            .iter()
            .filter(|o| o.base_priority > t.base_priority)
            .map(|o| (o.period.unwrap(), o.wcet))
            .collect();
        let r = response_time(t.wcet, period, &hp);
        if r.is_none() {
            schedulable = false;
        }
        response_times.push(r);
    }

    Analysis {
        utilization,
        utilization_bound,
        utilization_ok,
        response_times,
        schedulable,
    }
}

/// A one-line report of an analysis, for the CLI.
pub fn report(tasks: &[Task], a: &Analysis) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "utilization {:.3} / bound {:.3} ({})\n",
        a.utilization,
        a.utilization_bound,
        if a.utilization_ok { "within" } else { "over" }
    ));
    out.push_str("  id  name        prio  period  wcet  response  verdict\n");
    let periodic: Vec<&Task> = tasks.iter().filter(|t| t.period.is_some()).collect();
    for (t, r) in periodic.iter().zip(&a.response_times) {
        let (resp, verdict) = match r {
            Some(v) => (v.to_string(), if *v <= t.period.unwrap() { "ok" } else { "MISS" }),
            None => ("inf".to_string(), "MISS"),
        };
        out.push_str(&format!(
            "  {:>2}  {:<10}  {:>4}  {:>6}  {:>4}  {:>8}  {}\n",
            t.id,
            t.name,
            t.base_priority,
            t.period.unwrap(),
            t.wcet,
            resp,
            verdict,
        ));
    }
    out.push_str(&format!(
        "RTA verdict: {}\n",
        if a.schedulable { "SCHEDULABLE" } else { "NOT schedulable" }
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::Op;

    #[test]
    fn single_task_response_is_its_wcet() {
        let t = Task::new(0, "a", 1).periodic(10, vec![Op::Compute(4)]);
        let a = analyze(std::slice::from_ref(&t));
        assert_eq!(a.response_times[0], Some(4));
        assert!(a.schedulable);
    }

    #[test]
    fn interference_stacks_from_higher_priority() {
        // hi: C=2 T=5 (prio 2), lo: C=3 T=10 (prio 1).
        // lo response: 3 + ceil(R/5)*2. R=3 -> 3+2=5 -> ceil(5/5)*2=2 -> 5. Fixed at 5.
        let hi = Task::new(0, "hi", 2).periodic(5, vec![Op::Compute(2)]);
        let lo = Task::new(1, "lo", 1).periodic(10, vec![Op::Compute(3)]);
        let a = analyze(&[hi, lo]);
        assert_eq!(a.response_times[0], Some(2));
        assert_eq!(a.response_times[1], Some(5));
        assert!(a.schedulable);
    }

    #[test]
    fn overload_is_unschedulable() {
        // Two tasks each demanding more than half: utilization > 1.
        let a1 = Task::new(0, "a", 2).periodic(10, vec![Op::Compute(6)]);
        let a2 = Task::new(1, "b", 1).periodic(10, vec![Op::Compute(6)]);
        let a = analyze(&[a1, a2]);
        assert!(!a.schedulable);
        assert!(!a.utilization_ok);
    }
}
