# lazy_async_promise: Simple primitives to manage tokio and egui
[![Documentation](https://docs.rs/lazy_async_promise/badge.svg)](https://docs.rs/lazy_async_promise)
![CI](https://github.com/ChrisRega/lazy_async_promise/actions/workflows/rust.yml/badge.svg?branch=main "CI")

This crate currently only features two simple primitives for getting computation time off the main thread using tokio:
- LazyVecPromise for a vector-backed storage which can be partially displayed
- LazyValuePromise for a single value future

As the name suggests both of them are lazily evaluated and nothing happens until polled for the first time.