#![crate_name = "lazy_async_promise"]
//! # Primitives for combining tokio and immediate mode guis
//! Currently, only two primitives are implemented:
//! - [`LazyVecPromise`]: A lazily evaluated, partially readable and async-enabled vector-backed promise
//! - [`LazyValuePromise`]: A lazily evaluated and async-enabled single value promise
//!
#![warn(missing_docs)]
#![warn(unused_qualifications)]
#![deny(deprecated)]

extern crate core;

mod immediatevalue;
mod lazyvalue;
mod lazyvec;

#[doc(inline)]
pub use lazyvec::LazyVecPromise;

#[doc(inline)]
pub use immediatevalue::ImmediateValuePromise;
pub use immediatevalue::ImmediateValueState;
pub use immediatevalue::ToDynSendBox;

#[doc(inline)]
pub use lazyvalue::LazyValuePromise;

use std::fmt::Debug;
#[derive(Clone, PartialEq, Eq, Debug)]
/// Represents a processing state.
pub enum DataState {
    /// You should never receive this, as poll automatically updates
    Uninitialized,
    /// Data is complete
    UpToDate,
    /// Data is not (completely) ready, depending on your implementation, you may be able to get partial results
    Updating,
    /// Some error occurred
    Error(String),
}

#[derive(Debug)]
/// The message-type to send from the updater to the main thread. There's only two variants,
/// `NewData` which allows to send new data or `StateChange` which allows to signal readiness or error.
pub enum Message<T: Debug> {
    /// Adding or setting new data to the promise, depending on the implementation
    NewData(T),
    /// Modify the state of the promise, including setting an error
    StateChange(DataState),
}

/// Maybe this should rather be called "LazyUpdating"?
/// Implementors can react to polling by queueing an update if needed.
/// Update should force an update.
pub trait Promise {
    /// Polls the promise, triggers update if state is [`DataState::Uninitialized`]
    fn poll_state(&mut self) -> &DataState;
    /// Clears the data cache and immediately triggers an update
    fn update(&mut self);
}

/// Implementors can be viewed as slice
pub trait Sliceable<T> {
    /// get current data slice
    fn as_slice(&self) -> &[T];
}

/// Implementors may have a value
pub trait Value<T> {
    /// Maybe returns the value, depending on the object's state
    fn value(&self) -> Option<&T>;
}

/// Some type that implements lazy updating and provides a slice of the desired type
pub trait SlicePromise<T>: Promise + Sliceable<T> {}

/// Some type that implements lazy updating and provides a single value of the desired type
pub trait ValuePromise<T>: Promise + Value<T> {}

#[macro_export]
/// Error checking in async updater functions is tedious - this helps out by resolving results and sending errors on error. Result will be unwrapped if no error occurs.
macro_rules! unpack_result {
    ( $result: expr, $sender: expr ) => {
        match $result {
            Ok(val) => val,
            Err(e) => {
                $sender
                    .send(Message::StateChange(DataState::Error(format!("{}", e))))
                    .await
                    .unwrap();
                return;
            }
        }
    };
}
