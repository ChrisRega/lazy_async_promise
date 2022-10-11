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
pub use immediatevalue::ToDynSendBox;

#[doc(inline)]
pub use lazyvalue::LazyValuePromise;

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use tokio::sync::mpsc::Sender;

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
