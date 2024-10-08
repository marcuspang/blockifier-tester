use futures::future;
use serde_json::Value;
use starknet::core::types::TransactionTraceWithHash;
use std::{fs::OpenOptions, path::PathBuf};
use tokio::{
    fs::OpenOptions as AsyncOpenOptions,
    io::{AsyncWriteExt, BufWriter},
};

use log::{debug, info, warn};

use crate::juno_manager::ManagerError;

pub fn succesful_comparison_path(block_num: u64) -> PathBuf {
    PathBuf::from(format!("results/comparison-{}.json", block_num))
}

pub fn crashed_comparison_path(block_num: u64) -> PathBuf {
    PathBuf::from(format!("results/crash-{}.json", block_num))
}

pub fn unexpected_error_comparison_path(block_num: u64) -> PathBuf {
    PathBuf::from(format!("results/unexpected-error-{}.json", block_num))
}

pub fn base_trace_path(block_num: u64) -> PathBuf {
    PathBuf::from(format!("results/base/trace-{}.json", block_num))
}

// Creates a file in ./results/trace-{`block_number`}.json with the a full trace comparison between
// base blockifier and native blockifier results
pub async fn log_comparison_report(block_number: u64, comparison: Value) {
    let mut buffer = Vec::new();
    serde_json::to_writer_pretty(&mut buffer, &comparison).unwrap();

    let log_file = AsyncOpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(succesful_comparison_path(block_number))
        .await
        .expect("Failed to open log file");

    let mut writer = BufWriter::new(log_file);
    writer.write_all(&buffer).await.unwrap();
    writer.flush().await.unwrap();
}

pub fn log_crash_report(block_number: u64, report: serde_json::Value) {
    info!("Log report for block {block_number}");
    let block_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(crashed_comparison_path(block_number))
        .expect("Failed to open log file");

    serde_json::to_writer_pretty(block_file, &report)
        .unwrap_or_else(|_| panic!("failed to write block: {block_number}"));
}

// Creates a file in ./results/crash-{`block_number`}.json with the failure reason
pub async fn log_unexpected_error_report(block_number: u64, err: &ManagerError) {
    let mut log_file = AsyncOpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(unexpected_error_comparison_path(block_number))
        .await
        .expect("Failed to open log file");
    if let Err(write_err) = log_file.write_all(format!("{err:?}").as_bytes()).await {
        warn!("Failed to write err with error: '{write_err}'");
    }
}

// Creates a file in ./results/base/trace-{`block_number`}.json with the block trace by Base Juno
pub fn log_base_trace(block_number: u64, trace: &Vec<TransactionTraceWithHash>) {
    info!("Log trace for block {block_number}");

    let block_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(base_trace_path(block_number))
        .expect("Failed to open log file");

    serde_json::to_writer_pretty(block_file, trace)
        .unwrap_or_else(|_| panic!("failed to write block: {block_number}"));
}

/// Read a cached base trace
///
/// Returns None if no valid cached base trace is found.
/// todo(xrvdg) convert to result once blockifier has been parallelized?
pub fn read_base_trace(block_number: u64) -> Option<Vec<TransactionTraceWithHash>> {
    let block_file = OpenOptions::new()
        .read(true)
        .open(base_trace_path(block_number))
        .ok()?;

    // If parsing fails the file format has changed and the cache is invalidated
    serde_json::from_reader(block_file).ok()?
}

pub async fn prepare_directories() {
    debug!("Preparing directories");
    let paths = [
        "./results",
        "./cache",
        "./results/base",
        "./results/native",
        "./results/dependencies",
    ];

    future::join_all(paths.iter().map(tokio::fs::create_dir_all)).await;
}
