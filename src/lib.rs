#![crate_name = "lazy_async_promise"]
//! # Primitives for combining tokio and immediate mode guis
//! Currently, only three primitives are implemented:
//! - [`LazyVecPromise`]: A lazily evaluated, partially readable and async-enabled vector-backed promise
//! - [`LazyValuePromise`]: A lazily evaluated and async-enabled single value promise
//! - [`ImmediateValuePromise`]: An immediately updating async-enabled single value promise
//! See these items for their respective documentation. A general usage guide would be:
//! - You want several items of the same kind displayed / streamed? Use: [`LazyVecPromise`]
//! - You want one item displayed when ready and need lazy evaluation or have intermediate results? Use: [`LazyVecPromise`]
//! - You want one item displayed when ready and can afford spawning it directly? Use: [`ImmediateValuePromise`]
#![deny(missing_docs)]
#![deny(unused_qualifications)]
#![deny(deprecated)]
#![deny(absolute_paths_not_starting_with_crate)]
#![deny(unstable_features)]

extern crate core;

mod immediatevalue;
mod lazyvalue;
mod lazyvec;

#[doc(inline)]
pub use lazyvec::LazyVecPromise;

#[doc(inline)]
pub use immediatevalue::ImmediateValuePromise;
pub use immediatevalue::ImmediateValueState;

#[doc(inline)]
pub use lazyvalue::LazyValuePromise;

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::Sender;

/// a f32 type which is constrained to the range of 0.0 and 1.0
#[derive(Clone, PartialEq, Debug)]
pub struct Progress(f64);

impl<T: Into<f64>> From<T> for Progress {
    fn from(t: T) -> Self {
        Progress(t.into().clamp(0.0, 1.0))
    }
}

impl Progress {
    /// Create a Progress from a percentage
    /// ```rust, no_run
    /// use lazy_async_promise::Progress;
    /// let progress_half = Progress::from_percent(50);
    /// ```
    ///
    pub fn from_percent(percent: impl Into<f64>) -> Progress {
        Self::from_fraction(percent, 100.)
    }

    /// Create a Progress from a fraction, useful for handling loops
    /// ```rust, no_run
    /// use lazy_async_promise::Progress;
    /// let num_iterations = 100;
    /// for i in 0..num_iterations {
    ///   let progress_current = Progress::from_fraction(i, num_iterations);
    /// }
    /// ```
    ///
    pub fn from_fraction(numerator: impl Into<f64>, denominator: impl Into<f64>) -> Progress {
        (numerator.into() / denominator.into()).into()
    }
}

#[derive(Clone, PartialEq, Debug)]
/// Represents a processing state.
pub enum DataState {
    /// You should never receive this, as poll automatically updates
    Uninitialized,
    /// Data is complete
    UpToDate,
    /// Data is not (completely) ready, depending on your implementation, you may be able to get partial results
    /// Embedded progress in [0,1)
    Updating(Progress),
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

#[macro_export]
/// Setting the given progress using a given sender.
macro_rules! set_progress {
    ($progress: expr, $sender: expr) => {
        $sender
            .send(Message::StateChange(DataState::Updating($progress)))
            .await
            .unwrap();
    };
}

type BoxedFutureFactory<T> =
    Box<dyn Fn(Sender<Message<T>>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>>;

fn box_future_factory<
    T: Debug,
    U: Fn(Sender<Message<T>>) -> Fut + 'static,
    Fut: Future<Output = ()> + Send + 'static,
>(
    future_factory: U,
) -> BoxedFutureFactory<T> {
    Box::new(move |tx: Sender<Message<T>>| Box::pin(future_factory(tx)))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn progress_constructors() {
        let half = Progress::from_percent(50);
        assert_eq!(half.0, 0.5);
        let half = Progress::from_fraction(1, 2);
        assert_eq!(half.0, 0.5);
    }

    #[test]
    fn progress_clamps() {
        let minimum = Progress::from_percent(-50);
        assert_eq!(minimum.0, 0.0);
        let maximum = Progress::from_fraction(2, 1);
        assert_eq!(maximum.0, 1.0);
    }
}
