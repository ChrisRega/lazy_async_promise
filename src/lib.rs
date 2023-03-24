#![crate_name = "lazy_async_promise"]
//! # Primitives for combining tokio and immediate mode guis
//! Currently, only three primitives are implemented:
//! - [`LazyVecPromise`]: A lazily evaluated, partially readable and async-enabled vector-backed promise
//! - [`LazyValuePromise`]: A lazily evaluated and async-enabled single value promise
//! - [`ImmediateValuePromise`]: An immediately updating async-enabled single value promise
//! See these items for their respective documentation. A general usage guide would be:
//! - You want several items of the same kind displayed / streamed? Use: [`LazyVecPromise`]
//! - You want one item displayed when ready and need lazy evaluation or have intermediate results? Use: [`LazyValuePromise`]
//! - You just want one item displayed when ready? Use: [`ImmediateValuePromise`] (for laziness wrap in `Option`)
#![deny(missing_docs)]
#![deny(unused_qualifications)]
#![deny(deprecated)]
#![deny(absolute_paths_not_starting_with_crate)]
#![deny(unstable_features)]
#![deny(unsafe_code)]

extern crate core;

use std::error::Error;
use std::fmt::Debug;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;

use tokio::sync::mpsc::Sender;

#[doc(inline)]
pub use immediatevalue::ImmediateValuePromise;
pub use immediatevalue::ImmediateValueState;
#[doc(inline)]
pub use lazyvalue::LazyValuePromise;
#[doc(inline)]
pub use lazyvec::LazyVecPromise;

mod immediatevalue;
mod lazyvalue;
mod lazyvec;

/// Strong type to keep the boxed error. You can just deref it to get the inside box.
pub struct BoxedSendError(pub Box<dyn Error + Send>);


/// Type alias for futures with BoxedSendError
pub type FutureResult<T> = Result<T, BoxedSendError>;

impl<E: Error + Send + 'static> From<E> for BoxedSendError {
    fn from(e: E) -> Self {
        BoxedSendError(Box::new(e))
    }
}

impl Deref for BoxedSendError {
    type Target = Box<dyn Error + Send>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}


/// Trait for directly accessing the cache underneath any promise
pub trait DirectCacheAccess<T> {
    /// returns mutable reference to the cache if applicable
    fn get_value_mut(&mut self) -> Option<&mut T>;
    /// returns a reference to the cache if applicable
    fn get_value(&self) -> Option<&T>;
    /// takes the value and leaves the promise in a valid state indicating its emptiness
    fn take_value(&mut self) -> Option<T>;
}

/// Blanket implementation for any `Option<DirectCacheAccess<T>>` allows for better handling of option-laziness
impl<T: Send + 'static, A: DirectCacheAccess<T>> DirectCacheAccess<T> for Option<A> {
    fn get_value_mut(&mut self) -> Option<&mut T> {
        self.as_mut().and_then(|inner| inner.get_value_mut())
    }
    fn get_value(&self) -> Option<&T> {
        self.as_ref().and_then(|inner| inner.get_value())
    }
    fn take_value(&mut self) -> Option<T> {
        self.as_mut().and_then(|inner| inner.take_value())
    }
}

/// a f64 type which is constrained to the range of 0.0 and 1.0
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Progress(f64);

impl<T: Into<f64>> From<T> for Progress {
    fn from(t: T) -> Self {
        Progress(t.into().clamp(0.0, 1.0))
    }
}

/// Use this to get all macros
pub mod api_macros {
    pub use crate::Progress;
    pub use crate::send_data;
    pub use crate::set_error;
    pub use crate::set_finished;
    pub use crate::set_progress;
    pub use crate::unpack_result;
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

    /// return progress as f32
    pub fn as_f32(&self) -> f32 {
        self.0 as f32
    }

    /// return progress as f64
    pub fn as_f64(&self) -> f64 {
        self.0
    }
}

impl Default for Progress {
    fn default() -> Self {
        Progress(0.0)
    }
}

#[derive(Clone, PartialEq, Debug)]
/// Represents a processing state.
pub enum DataState {
    /// You can only receive this after taking ownership of the data
    Uninitialized,
    /// Data is complete
    UpToDate,
    /// Data is not (completely) ready, depending on your implementation, you may be able to get partial results
    /// Embedded progress in [0,1)
    Updating(Progress),
    /// Some error occurred
    Error(String),
}

impl DataState {
    /// Yields the progress if state is `DataState::Updating` otherwise yields none, even if finished.
    pub fn get_progress(&self) -> Option<Progress> {
        match &self {
            DataState::Updating(progress) => Some(*progress),
            _ => None,
        }
    }
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
                set_error!(format!("{}", e), $sender);
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

#[macro_export]
/// Setting the given progress using a given sender.
macro_rules! set_error {
    ($error: expr, $sender: expr) => {
        $sender
            .send(Message::StateChange(DataState::Error($error)))
            .await
            .unwrap();
    };
}

#[macro_export]
/// Send new data via the sender
macro_rules! send_data {
    ($data: expr, $sender: expr) => {
        $sender.send(Message::NewData($data)).await.unwrap();
    };
}

#[macro_export]
/// Set state to `DataState::UpToDate`
macro_rules! set_finished {
    ($sender: expr) => {
        $sender
            .send(Message::StateChange(DataState::UpToDate))
            .await
            .unwrap();
    };
}

type BoxedFutureFactory<T> =
Box<dyn Fn(Sender<Message<T>>) -> Pin<Box<dyn Future<Output=()> + Send + 'static>>>;

fn box_future_factory<
    T: Debug,
    U: Fn(Sender<Message<T>>) -> Fut + 'static,
    Fut: Future<Output=()> + Send + 'static,
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
        assert_eq!(minimum.as_f32(), 0.0);
        let maximum = Progress::from_fraction(2, 1);
        assert_eq!(maximum.as_f64(), 1.0);
        let progress: Progress = 2.0.into();
        assert_eq!(progress.as_f64(), 1.0);
    }

    #[test]
    fn default_progress_is_start() {
        assert_eq!(Progress::default().as_f64(), 0.0);
    }
}
