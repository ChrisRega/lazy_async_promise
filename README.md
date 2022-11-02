# lazy_async_promise: Simple primitives to manage tokio and egui
[![Documentation](https://docs.rs/lazy_async_promise/badge.svg)](https://docs.rs/lazy_async_promise)
![CI](https://github.com/ChrisRega/lazy_async_promise/actions/workflows/rust.yml/badge.svg?branch=main "CI")
[![Coverage Status](https://coveralls.io/repos/github/ChrisRega/lazy_async_promise/badge.svg?branch=main)](https://coveralls.io/github/ChrisRega/lazy_async_promise?branch=main)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat)](LICENSE-MIT)


This crate currently only features simple primitives for getting computation time off the main thread using tokio:
- `LazyVecPromise` for a vector-backed storage which can be displayed while the task in progress.
- `LazyValuePromise` for a single value future that can be updated during task-progress. My usage was for iterative algorithms where the intermediate results were interesting for display.

As the name suggests the two of them are lazily evaluated and nothing happens until they are polled for the first time.

For single values which are either available or not there's `ImmediateValuePromise` which triggers computation immediately.
There's not in-calculation value read out, so either it's finished or not. 

Example-usage of this crate with a small egui/eframe blog-reader can be found [here](https://github.com/ChrisRega/example-blog-client/)


Changelog:

0.3.1:
- Add better api macros for lazy structures

0.3.0:
- Removed `into_boxed` trait in favor of supporting the regular `From` trait which allows direct usage of the ?-Operator in `ImmediateValuePromise`
- Added a progress indicator for the `LazyVecPromise` and `LazyValuePromise`
- Indication is done by `Progress` strong type which ensures values between 0.0 and 1.0
- Added CI for mac, windows and linux - had to give test-timings more leeway for mac os than on linux.