use crate::{
    box_future_factory, BoxedFutureFactory, DataState, DirectCacheAccess, Message, Promise,
};
use std::fmt::Debug;
use std::future::Future;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// # A single lazy-async updated value
/// Create one with the [`LazyValuePromise::new`] method and supply an updater.
/// It's Updated only on first try to poll it making it scale nicely on more complex UIs.
/// While still updating, the value can be read out already, this can make sense for iterative algorithms where an intermediate result can be used as a preview.
/// Examples:
/// ```rust, no_run
/// use std::time::Duration;
/// use tokio::sync::mpsc::Sender;
/// use lazy_async_promise::{DirectCacheAccess, DataState, Message, Promise, LazyValuePromise, api_macros::*};
/// // updater-future:
/// let updater = |tx: Sender<Message<i32>>| async move {
///   send_data!(1337, tx);
///   // how to handle results and propagate the error to the future? Use `unpack_result!`:
///   let string = unpack_result!(std::fs::read_to_string("whatever.txt"), tx);
///   tokio::time::sleep(Duration::from_millis(100)).await;
///   set_finished!(tx);
/// };
/// // direct usage:
/// let promise = LazyValuePromise::new(updater, 10);
/// // for usage of the progress, see the docs of [`LazyVecPromise`]
/// fn main_loop(mut  lazy_promise: LazyValuePromise<i32>) {
///   loop {
///     match lazy_promise.poll_state() {
///       DataState::Error(er)  => { println!("Error {} occurred! Retrying!", er); std::thread::sleep(Duration::from_millis(500)); lazy_promise.update(); }
///       DataState::UpToDate   => { println!("Value up2date: {}", lazy_promise.get_value().unwrap()); }
///                           _ => { println!("Still updating... might be in strange state! (current state: {:?}", lazy_promise.get_value()); }
///     }
///   }
///
///   // Also, we can use all of DirectCacheAccess:
///    let current_cache = lazy_promise.get_value();
///    let current_cache_mut = lazy_promise.get_value_mut();
///    let current_cache_value = lazy_promise.take_inner();
/// }
/// ```
///
///

pub struct LazyValuePromise<T: Debug> {
    cache: Option<T>,
    updater: BoxedFutureFactory<T>,
    state: DataState,
    rx: Receiver<Message<T>>,
    tx: Sender<Message<T>>,
}
impl<T: Debug> LazyValuePromise<T> {
    /// Creates a new LazyValuePromise given an Updater and a tokio buffer size
    pub fn new<
        U: Fn(Sender<Message<T>>) -> Fut + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    >(
        future_factory: U,
        buffer_size: usize,
    ) -> Self {
        let (tx, rx) = channel::<Message<T>>(buffer_size);

        Self {
            cache: None,
            state: DataState::Uninitialized,
            rx,
            tx,
            updater: box_future_factory(future_factory),
        }
    }

    #[cfg(test)]
    pub fn is_uninitialized(&self) -> bool {
        self.state == DataState::Uninitialized
    }
}

impl<T: Debug> DirectCacheAccess<T> for LazyValuePromise<T> {
    /// get current value (may be incomplete) as mutable ref, be careful with this as
    /// further modification from the future may still push data.
    fn get_value_mut(&mut self) -> Option<&mut T> {
        self.cache.as_mut()
    }

    /// get current value, may be incomplete depending on status
    fn get_value(&self) -> Option<&T> {
        self.cache.as_ref()
    }

    /// takes the current value, if data was [`DataState::UpToDate`] it returns the value and sets the state to
    /// [`DataState::Uninitialized`]. Otherwise, returns None.
    fn take_inner(&mut self) -> Option<T> {
        if self.state == DataState::UpToDate {
            self.state = DataState::Uninitialized;
            self.cache.take()
        } else {
            None
        }
    }
}

impl<T: Debug> Promise for LazyValuePromise<T> {
    fn poll_state(&mut self) -> &DataState {
        if self.state == DataState::Uninitialized {
            self.update();
        }

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Message::NewData(data) => {
                    self.cache = Some(data);
                }
                Message::StateChange(new_state) => {
                    self.state = new_state;
                }
            }
        }

        &self.state
    }

    fn update(&mut self) {
        if matches!(self.state, DataState::Updating(_)) {
            return;
        }
        self.cache = None;

        self.state = DataState::Updating(0.0.into());
        let future = (self.updater)(self.tx.clone());
        tokio::spawn(future);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::api_macros::*;
    use std::time::Duration;
    use tokio::runtime::{Builder, Runtime};
    use tokio::sync::mpsc::Sender;

    #[test]
    fn basic_usage_cycle() {
        let string_maker = |tx: Sender<Message<String>>| async move {
            for i in 0..2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                send_data!(i.to_string(), tx);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            set_finished!(tx);
        };

        Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .max_blocking_threads(2)
            .build()
            .unwrap()
            .block_on(async {
                let mut delayed_value = LazyValuePromise::new(string_maker, 6);
                //be lazy:
                assert!(delayed_value.is_uninitialized());
                //start sets updating
                assert_eq!(
                    delayed_value.poll_state().get_progress().unwrap().as_f64(),
                    0.0
                );
                assert!(delayed_value.get_value().is_none());
                //after wait, value is there
                tokio::time::sleep(Duration::from_millis(150)).await;
                assert_eq!(*delayed_value.poll_state(), DataState::UpToDate);
                assert_eq!(delayed_value.get_value().unwrap(), "1");
                //update resets
                delayed_value.update();
                assert_eq!(
                    delayed_value.poll_state().get_progress().unwrap().as_f64(),
                    0.0
                );
                assert!(delayed_value.get_value().is_none());
                //after wait, value is there again and identical
                tokio::time::sleep(Duration::from_millis(150)).await;
                assert_eq!(*delayed_value.poll_state(), DataState::UpToDate);
                assert!(delayed_value.poll_state().get_progress().is_none());
                assert_eq!(delayed_value.get_value().unwrap(), "1");
            });
    }

    #[test]
    fn error_propagation() {
        let error_maker = |tx: Sender<Message<String>>| async move {
            let _ = unpack_result!(std::fs::read_to_string("FILE_NOT_EXISTING"), tx);
            unreachable!();
        };

        Runtime::new().unwrap().block_on(async {
            let mut delayed_vec = LazyValuePromise::new(error_maker, 1);
            assert_eq!(*delayed_vec.poll_state(), DataState::Updating(0.0.into()));
            assert!(delayed_vec.get_value().is_none());
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert!(matches!(*delayed_vec.poll_state(), DataState::Error(_)));
            assert!(delayed_vec.get_value().is_none());
        });
    }

    #[test]
    fn test_direct_cache_access() {
        let int_maker = |tx: Sender<Message<i32>>| async move {
            send_data!(42, tx);
            set_finished!(tx);
        };

        Runtime::new().unwrap().block_on(async {
            let mut delayed_value = LazyValuePromise::new(int_maker, 6);
            let _ = delayed_value.poll_state();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let poll_result = delayed_value.poll_state();
            assert!(matches!(poll_result, DataState::UpToDate));
            let val = delayed_value.get_value();
            assert_eq!(*val.unwrap(), 42);
            let _val_mut = delayed_value.get_value_mut();
            let value_owned = delayed_value.take_inner().unwrap();
            assert_eq!(value_owned, 42);
            assert!(delayed_value.is_uninitialized());
        });
    }
}
