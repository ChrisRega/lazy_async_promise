use crate::{DataState, Message, Promise, Value, ValuePromise};
use std::fmt::Debug;
use std::future::Future;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// # A single lazy-async updated value
/// Create one with the [`LazyValuePromise::new`] method and supply an updater.
/// It's Updated only on first try to poll it making it scale nicely on more complex UIs.
/// Type erasure can be done using a Box with dyn [`ValuePromise`]
/// Examples:
/// ```rust, no_run
/// use std::time::Duration;
/// use tokio::sync::mpsc::Sender;
/// use lazy_async_promise::{DataState, Message, ValuePromise};
/// use lazy_async_promise::LazyValuePromise;
/// use lazy_async_promise::unpack_result;
/// // updater-future:
/// let updater = |tx: Sender<Message<i32>>| async move {
///   tx.send(Message::NewData(1337)).await.unwrap();
///   // how to handle results and propagate the error to the future? Use `unpack_result!`:
///   let string = unpack_result!(std::fs::read_to_string("whatever.txt"), tx);///
///   tokio::time::sleep(Duration::from_millis(100)).await;
///   tx.send(Message::StateChange(DataState::UpToDate)).await.unwrap();
/// };
/// // direct usage:
/// let promise = LazyValuePromise::new(updater, 10);
/// // or storing it with type-erasure for easier usage in application state structs:
/// let boxed: Box<dyn ValuePromise<i32>> = Box::new(LazyValuePromise::new(updater, 6));
///
/// fn main_loop(lazy_promise: &mut Box<dyn ValuePromise<i32>>) {
///   loop {
///     match lazy_promise.poll_state() {
///       DataState::Error(er)  => { println!("Error {} occurred! Retrying!", er); std::thread::sleep(Duration::from_millis(500)); lazy_promise.update(); },
///       DataState::UpToDate   => { println!("Value up2date: {}", lazy_promise.value().unwrap()); },
///                           _ => { println!("Still updating... might be in strange state! (current state: {:?}", lazy_promise.value()); }  
///     }
///   }
/// }
/// ```
///
///

pub struct LazyValuePromise<
    T: Debug,
    U: Fn(Sender<Message<T>>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
> {
    cache: Option<T>,
    updater: U,
    state: DataState,
    rx: Receiver<Message<T>>,
    tx: Sender<Message<T>>,
}
impl<T: Debug, U: Fn(Sender<Message<T>>) -> Fut, Fut: Future<Output = ()> + Send + 'static>
    LazyValuePromise<T, U, Fut>
{
    /// Creates a new LazyValuePromise given an Updater and a tokio buffer size
    pub fn new(updater: U, buffer_size: usize) -> Self {
        let (tx, rx) = channel::<Message<T>>(buffer_size);

        Self {
            cache: None,
            state: DataState::Uninitialized,
            rx,
            tx,
            updater,
        }
    }
}

impl<T: Debug, U: Fn(Sender<Message<T>>) -> Fut, Fut: Future<Output = ()> + Send + 'static> Value<T>
    for LazyValuePromise<T, U, Fut>
{
    fn value(&self) -> Option<&T> {
        self.cache.as_ref()
    }
}

impl<T: Debug, U: Fn(Sender<Message<T>>) -> Fut, Fut: Future<Output = ()> + Send + 'static>
    ValuePromise<T> for LazyValuePromise<T, U, Fut>
{
}

impl<T: Debug, U: Fn(Sender<Message<T>>) -> Fut, Fut: Future<Output = ()> + Send + 'static> Promise
    for LazyValuePromise<T, U, Fut>
{
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
        if self.state == DataState::Updating {
            return;
        }
        self.cache = None;

        self.state = DataState::Updating;
        let future = (self.updater)(self.tx.clone());
        tokio::spawn(future);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::unpack_result;
    use std::time::Duration;
    use tokio::runtime::Runtime;
    use tokio::sync::mpsc::Sender;

    #[test]
    fn basic_usage_cycle() {
        let string_maker = |tx: Sender<Message<String>>| async move {
            for i in 0..2 {
                tx.send(Message::NewData(i.to_string())).await.unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tx.send(Message::StateChange(DataState::UpToDate))
                .await
                .unwrap();
        };

        Runtime::new().unwrap().block_on(async {
            let mut delayed_value = LazyValuePromise::new(string_maker, 6);
            //start empty
            assert_eq!(*delayed_value.poll_state(), DataState::Updating);
            assert!(delayed_value.value().is_none());
            //after wait, value is there
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(*delayed_value.poll_state(), DataState::UpToDate);
            assert_eq!(delayed_value.value().unwrap(), "1");
            //update resets
            delayed_value.update();
            assert_eq!(*delayed_value.poll_state(), DataState::Updating);
            assert!(delayed_value.value().is_none());
            //after wait, value is there again and identical
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(*delayed_value.poll_state(), DataState::UpToDate);
            assert_eq!(delayed_value.value().unwrap(), "1");
        });
    }

    #[test]
    fn error_propagation() {
        let error_maker = |tx: Sender<Message<String>>| async move {
            let _ = unpack_result!(std::fs::read_to_string("FILE_NOT_EXISTING"), tx);
            tx.send(Message::StateChange(DataState::UpToDate))
                .await
                .unwrap();
        };

        Runtime::new().unwrap().block_on(async {
            let mut delayed_vec = LazyValuePromise::new(error_maker, 1);
            assert_eq!(*delayed_vec.poll_state(), DataState::Updating);
            assert!(delayed_vec.value().is_none());
            std::thread::sleep(Duration::from_millis(150));
            assert!(matches!(*delayed_vec.poll_state(), DataState::Error(_)));
            assert!(delayed_vec.value().is_none());
        });
    }
}
