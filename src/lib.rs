#[cfg(all(feature = "moka-v010", any(feature = "moka-v09", feature = "moka-v08")))]
compile_error!(
    "You cannot enable `moka-v09` and/or `moka-v8` features while `moka-v010` is enabled.\n\
                You might need `--no-default-features`."
);

use std::io::prelude::*;
use std::sync::Arc;
use std::{fs::File, io::BufReader, time::Instant};

#[cfg(feature = "moka-v010")]
pub(crate) use moka010 as moka;

#[cfg(feature = "moka-v09")]
pub(crate) use moka09 as moka;

#[cfg(feature = "moka-v08")]
pub(crate) use moka08 as moka;

mod cache;
pub mod config;
mod eviction_counters;
mod load_gen;
mod parser;
mod report;
mod trace_file;

pub(crate) use eviction_counters::EvictionCounters;
pub use report::Report;
pub use trace_file::TraceFile;

use cache::{
    moka_driver::{
        async_cache::MokaAsyncCache, sync_cache::MokaSyncCache, sync_segmented::MokaSegmentedCache,
    },
    AsyncCacheDriver, CacheDriver,
};
use config::Config;
use itertools::Itertools;
use parser::TraceEntry;
use report::ReportBuilder;

#[cfg(feature = "hashlink")]
use crate::cache::hashlink::HashLink;
#[cfg(any(feature = "mini-moka", feature = "moka-v08", feature = "moka-v09"))]
use crate::cache::mini_moka_driver::{
    sync_cache::MiniMokSyncCache, unsync_cache::MiniMokaUnsyncCache,
};
#[cfg(feature = "quick_cache")]
use crate::cache::quick_cache::QuickCache;
#[cfg(feature = "stretto")]
use crate::cache::stretto::StrettoCache;

const BATCH_SIZE: usize = 200;

pub(crate) enum Command {
    GetOrInsert(String, usize),
    GetOrInsertOnce(String, usize),
    Update(String, usize),
    Invalidate(String, usize),
    InvalidateAll,
    InvalidateEntriesIf(String, usize),
    Iterate,
}

pub fn run_multi_threads_moka_sync(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let max_cap = if config.size_aware {
        capacity as u64 * 2u64.pow(15)
    } else {
        capacity as u64
    };
    let cache_driver = MokaSyncCache::new(config, max_cap, capacity);
    let report_builder = ReportBuilder::new("Moka Sync Cache", max_cap, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

pub fn run_multi_threads_moka_segment(
    config: &Config,
    capacity: usize,
    num_clients: u16,
    num_segments: usize,
) -> anyhow::Result<Report> {
    let max_cap = if config.size_aware {
        capacity as u64 * 2u64.pow(15)
    } else {
        capacity as u64
    };
    let cache_driver = MokaSegmentedCache::new(config, max_cap, capacity, num_segments);
    let report_name = format!("Moka SegmentedCache({num_segments})");
    let report_builder = ReportBuilder::new(&report_name, max_cap, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

pub async fn run_multi_tasks_moka_async(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let max_cap = if config.size_aware {
        capacity as u64 * 2u64.pow(15)
    } else {
        capacity as u64
    };
    let cache_driver = MokaAsyncCache::new(config, max_cap, capacity);
    let report_builder = ReportBuilder::new("Moka Async Cache", max_cap, Some(num_clients));
    run_multi_tasks(config, num_clients, cache_driver, report_builder).await
}

#[cfg(any(feature = "mini-moka", feature = "moka-v08", feature = "moka-v09"))]
pub fn run_multi_threads_moka_dash(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let max_cap = if config.size_aware {
        capacity as u64 * 2u64.pow(15)
    } else {
        capacity as u64
    };
    let cache_driver = MiniMokSyncCache::new(config, max_cap, capacity);
    let report_name = if cfg!(feature = "mini-moka") {
        "Mini Moka Sync Cache"
    } else {
        "Moka Dash Cache"
    };
    let report_builder = ReportBuilder::new(report_name, max_cap, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "hashlink")]
pub fn run_multi_threads_hashlink(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let cache_driver = HashLink::new(config, capacity);
    let report_builder = ReportBuilder::new("HashLink", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "quick_cache")]
#[allow(clippy::needless_collect)] // on the `handles` variable.
pub fn run_multi_threads_quick_cache(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let cache_driver = QuickCache::new(config, capacity);
    let report_builder = ReportBuilder::new("QuickCache", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "stretto")]
#[allow(clippy::needless_collect)] // on the `handles` variable.
pub fn run_multi_threads_stretto(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let cache_driver = StrettoCache::new(config, capacity);
    let report_builder = ReportBuilder::new("Stretto", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(any(feature = "mini-moka", feature = "moka-v08", feature = "moka-v09"))]
pub fn run_single(config: &Config, capacity: usize) -> anyhow::Result<Report> {
    let mut max_cap = capacity.try_into().unwrap();
    if config.size_aware {
        max_cap *= 2u64.pow(15);
    }
    let mut cache_driver = MiniMokaUnsyncCache::new(config, max_cap, capacity);
    let name = if cfg!(feature = "mini-moka") {
        "Mini Moka Unsync Cache"
    } else {
        "Moka Unsync Cache"
    };
    let mut report = Report::new(name, max_cap, Some(1));
    let mut counter = 0;

    let instant = Instant::now();

    for _ in 0..(config.repeat.unwrap_or(1)) {
        let f = File::open(config.trace_file.path())?;
        let reader = BufReader::new(f);

        for chunk in reader.lines().chunks(BATCH_SIZE).into_iter() {
            let commands = load_gen::generate_commands(config, BATCH_SIZE, &mut counter, chunk)?;
            cache::process_commands(commands, &mut cache_driver, &mut report);
        }
    }

    let elapsed = instant.elapsed();
    report.duration = Some(elapsed);

    Ok(report)
}

#[allow(clippy::needless_collect)] // on the `handles` variable.
fn run_multi_threads(
    config: &Config,
    num_clients: u16,
    cache_driver: impl CacheDriver<TraceEntry> + Clone + Send + 'static,
    report_builder: ReportBuilder,
) -> anyhow::Result<Report> {
    let report_builder = Arc::new(report_builder);
    let (send, receive) = crossbeam_channel::bounded::<Vec<Command>>(100);

    let handles = (0..num_clients)
        .map(|_| {
            let mut cache = cache_driver.clone();
            let ch = receive.clone();
            let rb = Arc::clone(&report_builder);

            std::thread::spawn(move || {
                let mut report = rb.build();
                while let Ok(commands) = ch.recv() {
                    cache::process_commands(commands, &mut cache, &mut report);
                }
                report
            })
        })
        .collect::<Vec<_>>();

    let mut counter = 0;
    let instant = Instant::now();

    for _ in 0..(config.repeat.unwrap_or(1)) {
        let f = File::open(config.trace_file.path())?;
        let reader = BufReader::new(f);
        for chunk in reader.lines().chunks(BATCH_SIZE).into_iter() {
            let commands = load_gen::generate_commands(config, BATCH_SIZE, &mut counter, chunk)?;
            send.send(commands)?;
        }
    }

    // Drop the sender channel to notify the workers that we are finished.
    std::mem::drop(send);

    // Wait for the workers to finish and collect their reports.
    let reports = handles
        .into_iter()
        .map(|h| h.join().expect("Failed"))
        .collect::<Vec<_>>();
    let elapsed = instant.elapsed();

    // Merge the reports into one.
    let mut report = report_builder.build();
    report.duration = Some(elapsed);
    reports.iter().for_each(|r| report.merge(r));

    if config.is_eviction_listener_enabled() {
        report.add_eviction_counts(cache_driver.eviction_counters().as_ref().unwrap());
    }

    Ok(report)
}

async fn run_multi_tasks(
    config: &Config,
    num_clients: u16,
    cache_driver: impl AsyncCacheDriver<TraceEntry> + Clone + Send + 'static,
    report_builder: ReportBuilder,
) -> anyhow::Result<Report> {
    let report_builder = Arc::new(report_builder);
    let (send, receive) = crossbeam_channel::bounded::<Vec<Command>>(100);

    let handles = (0..num_clients)
        .map(|_| {
            let mut cache = cache_driver.clone();
            let ch = receive.clone();
            let rb = Arc::clone(&report_builder);

            tokio::task::spawn(async move {
                let mut report = rb.build();
                while let Ok(commands) = ch.recv() {
                    cache::process_commands_async(commands, &mut cache, &mut report).await;
                }
                report
            })
        })
        .collect::<Vec<_>>();

    let mut counter = 0;
    let instant = Instant::now();

    for _ in 0..(config.repeat.unwrap_or(1)) {
        let f = File::open(config.trace_file.path())?;
        let reader = BufReader::new(f);
        for chunk in reader.lines().chunks(BATCH_SIZE).into_iter() {
            let commands = load_gen::generate_commands(config, BATCH_SIZE, &mut counter, chunk)?;
            send.send(commands)?;
        }
    }

    // Drop the sender channel to notify the workers that we are finished.
    std::mem::drop(send);

    // Wait for the workers to finish and collect their reports.
    let reports = futures_util::future::join_all(handles).await;
    let elapsed = instant.elapsed();

    // Merge the reports into one.
    let mut report = report_builder.build();
    report.duration = Some(elapsed);
    reports
        .iter()
        .for_each(|r| report.merge(r.as_ref().expect("Failed")));

    if config.is_eviction_listener_enabled() {
        report.add_eviction_counts(cache_driver.eviction_counters().as_ref().unwrap());
    }

    Ok(report)
}
