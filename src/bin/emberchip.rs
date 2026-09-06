//! Emberchip command-line runner.
//!
//! Subcommands:
//!   demo              Blinky, two mutex-sharing workers, and a preempting sensor.
//!   run [TICKS]       A random schedulable task set, run for TICKS ticks.
//!   inversion [on|off] The priority-inversion scenario, inheritance on or off.
//!
//! Flags:
//!   --seed N          Seed the simulation (default 1).
//!   --ticks N         Number of ticks (demo and inversion have sensible defaults).
//!   --quiet           Print only the summary, not the full timeline.

#![warn(clippy::pedantic)]
#![allow(
    clippy::must_use_candidate,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    clippy::doc_markdown
)]

use emberchip::{timeline, workload, Config, Rng};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map_or("demo", String::as_str);

    let mut seed = 1u64;
    let mut ticks: Option<u64> = None;
    let mut quiet = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--seed" => {
                i += 1;
                seed = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(seed);
            }
            "--ticks" => {
                i += 1;
                ticks = args.get(i).and_then(|v| v.parse().ok());
            }
            "--quiet" => quiet = true,
            _ => {}
        }
        i += 1;
    }

    match cmd {
        "demo" => run_demo(seed, ticks.unwrap_or(40), quiet),
        "run" => {
            // `run 200` positional ticks also accepted.
            let positional = args.get(1).and_then(|v| v.parse::<u64>().ok());
            run_random(seed, ticks.or(positional).unwrap_or(200), quiet);
        }
        "inversion" => {
            let mode = args.get(1).map_or("on", String::as_str);
            let inherit = mode != "off";
            run_inversion(seed, ticks.unwrap_or(30), inherit, quiet);
        }
        "analyze" => run_analyze(seed),
        "help" | "--help" | "-h" => print_help(),
        other => {
            eprintln!("unknown command: {other}\n");
            print_help();
            std::process::exit(2);
        }
    }
}

fn header(title: &str) {
    println!("Emberchip: deterministic RTOS simulator");
    println!("{title}");
    println!("(teaching-accurate model, not firmware for a real chip)");
}

fn run_demo(seed: u64, ticks: u64, quiet: bool) {
    header(&format!("demo, seed {seed}, {ticks} ticks"));
    let cfg = Config {
        seed,
        ..Config::default()
    };
    let mut k = workload::demo(cfg);
    k.run(ticks);
    if !quiet {
        print!("{}", timeline::render(&k));
    }
    print!("{}", timeline::summary(&k));
    println!("LED final state: {}", if k.mcu.gpio.led() { "on" } else { "off" });
}

fn run_random(seed: u64, ticks: u64, quiet: bool) {
    let mut rng = Rng::new(seed);
    let n = rng.range(3, 5) as usize;
    header(&format!("random schedulable set, seed {seed}, {n} tasks, {ticks} ticks"));
    let cfg = Config {
        seed,
        ..Config::default()
    };
    let mut k = emberchip::Kernel::new(cfg);
    for t in workload::random_schedulable_set(&mut rng, n) {
        k.add_task(t);
    }
    k.run(ticks);
    if !quiet {
        print!("{}", timeline::render(&k));
    }
    print!("{}", timeline::summary(&k));
    println!(
        "invariant violations: {}, deadline misses: {}",
        k.invariant_violations(),
        k.total_deadline_misses()
    );
}

fn run_inversion(seed: u64, ticks: u64, inherit: bool, quiet: bool) {
    header(&format!(
        "priority inversion, inheritance {}, seed {seed}, {ticks} ticks",
        if inherit { "ON" } else { "OFF" }
    ));
    let cfg = Config {
        seed,
        priority_inheritance: inherit,
        ..Config::default()
    };
    let mut k = workload::priority_inversion(cfg);
    k.run(ticks);
    if !quiet {
        print!("{}", timeline::render(&k));
    }
    print!("{}", timeline::summary(&k));
    let blocked = workload::high_blocking_ticks(&k);
    if blocked == u64::MAX {
        println!("high task never acquired the mutex within {ticks} ticks (unbounded inversion)");
    } else {
        println!("high task was blocked for {blocked} ticks waiting on the mutex");
    }
}

fn run_analyze(seed: u64) {
    use emberchip::schedulability;
    let mut rng = Rng::new(seed);
    let n = rng.range(2, 6) as usize;
    let tasks = workload::random_rm_set(&mut rng, n);
    header(&format!("rate-monotonic analysis, seed {seed}, {n} tasks"));
    let analysis = emberchip::analyze(&tasks);
    print!("{}", schedulability::report(&tasks, &analysis));

    // Confirm the prediction against the simulator: the analysis is exact for a
    // synchronous set, so the run must agree.
    let cfg = Config {
        seed,
        ..Config::default()
    };
    let mut k = emberchip::Kernel::new(cfg);
    for t in &tasks {
        k.add_task(t.clone());
    }
    let horizon = workload::hyperperiod(&k.tasks, 5_000).max(1_000);
    k.run(horizon);
    let misses = k.total_deadline_misses();
    let sim = misses == 0;
    println!(
        "simulated {horizon} ticks: {misses} deadline misses ({})",
        if sim { "met all deadlines" } else { "missed" }
    );
    println!(
        "analysis vs simulation: {}",
        if analysis.schedulable == sim { "AGREE" } else { "DISAGREE" }
    );
}

fn print_help() {
    println!(
        "emberchip: deterministic RTOS simulator\n\
         \n\
         USAGE:\n\
         \x20 emberchip demo [--seed N] [--ticks N] [--quiet]\n\
         \x20 emberchip run [TICKS] [--seed N] [--quiet]\n\
         \x20 emberchip inversion [on|off] [--seed N] [--ticks N] [--quiet]\n\
         \x20 emberchip analyze [--seed N]\n\
         \n\
         EMBERCHIP_FUZZ_OPS controls the randomized test budget (see cargo test)."
    );
}
