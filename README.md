## Purpose

This is a tool created for testing the [Blockifier integrated with Cairo Native](https://github.com/NethermindEth/blockifier).

## Project Status

The tool is a work in progress and is primarily used only during this stage to replay blocks. It currently has no guarantees of stability for any behavior.

## Terminology

Native Juno : Juno instance running with Native Blockifier

Base Juno   : Juno instance (used to distinguish between a "normal" Juno instance and Native Juno)

## What it does

The tool takes a block range (currently hard coded in main.rs). Note that for now, blocks are sorted in ascending order of how many transactions they have in order to avoid having to run many long RPC calls before we can get any results.

For each block it will:
Attempt to trace the block with Native Juno. If the trace had no failures* then the block will be traced with Base Juno and a comparison between the two results will be dumped in `./results/trace-<block_number>`. Otherwise, the block transactions will be simulated and a report will be dumped in `./results/block-<block_number>`. Currently, the block is simulated using a binary search to find which transaction crashes Juno.

*A failure in this case is defined as _any_ of the following:
1. Juno crashing
2. The block is not found (this likely means your Juno database did not have the block)

## Setup

### Dependencies (Juno: base and native)

To get your base version of Juno you need to first clone the [repo](https://github.com/NethermindEth/juno) and build it via `make juno`. Be sure to install all needed dependencies first, which are specified in the that same repository.

Then, to obtain the native version, clone the project again, _switch to `native2.6.3-blockifier` branch_ and recompile. Make sure you have `cairo_native` installed properly and the runtime lib is in your environment.

Finally, Juno must be in sync and have Starknet latest blockchain history. To achieve this you can either:

1. (recommended) Manually download a snapshot from [here](https://github.com/NethermindEth/juno). Be sure that the snapshot you are downloading is recent enough.

2. Sync Juno manually (around 4 to 5 days in optimal conditions)

### Config

In the `config.toml` located at the project root set the following variables*:

```toml
juno_path = "<path to base Juno executable>"
juno_native_path = "<path to native Juno executable>"
juno_database_path = "<path to Juno's database>" # correlates to `--db-path` argument passed to Juno
```

It is recommended that you use absolute paths and avoid `$HOME` and `~`

Example `config.toml`:
```toml
juno_path = "/home/pwhite/repos/juno/build/juno"
juno_native_path = "/home/pwhite/repos/native_juno/build/juno"
juno_database_path = "/home/pwhite/snapshots/juno_mainnet"
```

### Usage

Once setup is complete, build the project with:

```
cargo build
```

and/or directly run it with

```
cargo run
```


## Troubleshooting

Problem: A block fails on Juno Native when I don't expect it to.
Suggestion: Check `juno_out.log` to see what the actual failure was. If the failure was that the block was not found, check `juno_database_path` in your `Config.toml` and make sure it's pointing to a database path that has that block.