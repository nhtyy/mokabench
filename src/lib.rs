#[cfg(all(
    feature = "moka-v012",
    any(
        feature = "moka-v011",
        feature = "moka-v010",
        feature = "moka-v09",
        feature = "moka-v08"
    )
))]
compile_error!(
    "You cannot enable `moka-v011`, `moka-v010`, `moka-v09` and/or `moka-v8` features while `moka-v012` is enabled.\n\
                You might need `--no-default-features`."
);

use std::io::prelude::*;
use std::sync::Arc;
use std::{fs::File, io::BufReader, time::Instant};

#[cfg(feature = "moka-v012")]
pub(crate) use moka012 as moka;

#[cfg(feature = "moka-v011")]
pub(crate) use moka011 as moka;

#[cfg(feature = "moka-v010")]
pub(crate) use moka010 as moka;

#[cfg(feature = "moka-v09")]
pub(crate) use moka09 as moka;

#[cfg(feature = "moka-v08")]
pub(crate) use moka08 as moka;

mod async_rt_helper;
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

use async_rt_helper as rt;
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
#[cfg(feature = "tiny-ufo")]
use crate::cache::tiny_ufo::TinyUfoCache;

const BATCH_SIZE: usize = 200;

pub(crate) enum Command {
    GetOrInsert(TraceEntry),
    GetOrInsertOnce(TraceEntry),
    Update(TraceEntry),
    Invalidate(TraceEntry),
    InvalidateAll,
    InvalidateEntriesIf(TraceEntry),
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
    let report_builder = ReportBuilder::new("Moka Sync Cache", max_cap, Some(num_clients));

    #[cfg(not(any(feature = "moka-v08", feature = "moka-v09")))]
    if config.entry_api {
        let cache_driver = MokaSyncCache::with_entry_api(config, max_cap, capacity);
        return run_multi_threads(config, num_clients, cache_driver, report_builder);
    }

    let cache_driver = MokaSyncCache::new(config, max_cap, capacity);
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
    let report_name = format!("Moka SegmentedCache({num_segments})");
    let report_builder = ReportBuilder::new(&report_name, max_cap, Some(num_clients));

    #[cfg(not(any(feature = "moka-v08", feature = "moka-v09")))]
    if config.entry_api {
        let cache_driver =
            MokaSegmentedCache::with_entry_api(config, max_cap, capacity, num_segments);
        return run_multi_threads(config, num_clients, cache_driver, report_builder);
    }

    let cache_driver = MokaSegmentedCache::new(config, max_cap, capacity, num_segments);
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
    let report_builder = ReportBuilder::new("Moka Async Cache", max_cap, Some(num_clients));

    #[cfg(not(any(feature = "moka-v08", feature = "moka-v09")))]
    if config.entry_api {
        let cache_driver = MokaAsyncCache::with_entry_api(config, max_cap, capacity);
        return run_multi_tasks(config, num_clients, cache_driver, report_builder).await;
    }

    let cache_driver = MokaAsyncCache::new(config, max_cap, capacity);
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
    let report_builder =
        ReportBuilder::new("HashLink (LRU w/ Mutex)", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "quick_cache")]
pub fn run_multi_threads_quick_cache(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let max_cap = if config.size_aware {
        capacity as u64 * 2u64.pow(15)
    } else {
        capacity as u64
    };
    let cache_driver = QuickCache::new(config, capacity, max_cap);
    let report_builder =
        ReportBuilder::new("QuickCache Sync Cache", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "light-cache")]
pub fn run_multi_threads_light_cache(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    use cache::light_cache::LightCache;

    let cache_driver = LightCache::new(config, capacity);
    let report_builder =
        ReportBuilder::new("LightCache Sync Cache", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "light-cache-lru")]
pub fn run_multi_threads_light_cache_lru(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    use cache::light_cache_lru::LightCacheLru;

    let cache_driver = LightCacheLru::new(config, capacity);
    let report_builder =
        ReportBuilder::new("LightCache Sync Cache LRU", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "stretto")]
pub fn run_multi_threads_stretto(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let cache_driver = StrettoCache::new(config, capacity);
    let report_builder = ReportBuilder::new("Stretto", capacity as _, Some(num_clients));
    run_multi_threads(config, num_clients, cache_driver, report_builder)
}

#[cfg(feature = "tiny-ufo")]
pub fn run_multi_threads_tiny_ufo(
    config: &Config,
    capacity: usize,
    num_clients: u16,
) -> anyhow::Result<Report> {
    let cache_driver = TinyUfoCache::new(config, capacity);
    let report_builder = ReportBuilder::new("TinyUFO", capacity as _, Some(num_clients));
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

    // pre-process all commands to reduce benchmark harness influence.
    let mut all_commands = Vec::new();
    for _ in 0..(config.repeat.unwrap_or(1)) {
        let f = File::open(config.trace_file.path())?;
        let reader = BufReader::new(f);

        for chunk in reader.lines().enumerate().chunks(BATCH_SIZE).into_iter() {
            let chunk = chunk.map(|(i, r)| r.map(|s| (i, s)));
            let commands = load_gen::generate_commands(config, BATCH_SIZE, &mut counter, chunk)?;
            all_commands.push(commands);
        }
    }

    let instant = Instant::now();
    for commands in all_commands {
        cache::process_commands(commands, &mut cache_driver, &mut report);
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
    let (send, receive) = crossbeam_channel::unbounded::<Vec<Command>>();

    // In order to have the minimum harness overhead and not have many consumers
    // waiting for the single producer, we buffer all operations in a channel.
    let mut counter = 0;
    for _ in 0..(config.repeat.unwrap_or(1)) {
        let f = File::open(config.trace_file.path())?;
        let reader = BufReader::new(f);
        for chunk in reader.lines().enumerate().chunks(BATCH_SIZE).into_iter() {
            let chunk = chunk.map(|(i, r)| r.map(|s| (i, s)));
            let commands = load_gen::generate_commands(config, BATCH_SIZE, &mut counter, chunk)?;
            send.send(commands)?;
        }
    }

    // Drop the sender channel to notify the workers that we are finished.
    std::mem::drop(send);

    let instant = Instant::now();
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
    let (send, receive) = crossbeam_channel::unbounded::<Vec<Command>>();

    // In order to have the minimum harness overhead and not have many consumers
    // waiting for the single producer, we buffer all operations in a channel.
    let mut counter = 0;
    for _ in 0..(config.repeat.unwrap_or(1)) {
        let f = File::open(config.trace_file.path())?;
        let reader = BufReader::new(f);
        for chunk in reader.lines().enumerate().chunks(BATCH_SIZE).into_iter() {
            let chunk = chunk.map(|(i, r)| r.map(|s| (i, s)));
            let commands = load_gen::generate_commands(config, BATCH_SIZE, &mut counter, chunk)?;
            send.send(commands)?;
        }
    }

    // Drop the sender channel to notify the workers that we are finished.
    std::mem::drop(send);

    let instant = Instant::now();
    let handles = (0..num_clients)
        .map(|_| {
            let mut cache = cache_driver.clone();
            let ch = receive.clone();
            let rb = Arc::clone(&report_builder);
            let mut count = 0u32;

            rt::spawn(async move {
                let mut report = rb.build();
                while let Ok(commands) = ch.recv() {
                    cache::process_commands_async(commands, &mut cache, &mut report).await;
                    count += 1;
                    if count % 10_000 == 0 {
                        tokio::task::yield_now().await;
                    }
                }
                report
            })
        })
        .collect::<Vec<_>>();

    // Wait for the workers to finish and collect their reports.
    let reports = futures_util::future::join_all(handles).await;
    let elapsed = instant.elapsed();

    // Merge the reports into one.
    let mut report = report_builder.build();
    report.duration = Some(elapsed);

    for r in reports {
        #[cfg(feature = "rt-tokio")]
        report.merge(&r.expect("Failed"));

        #[cfg(feature = "rt-async-std")]
        report.merge(&r);
    }

    if config.is_eviction_listener_enabled() {
        report.add_eviction_counts(cache_driver.eviction_counters().as_ref().unwrap());
    }

    Ok(report)
}
