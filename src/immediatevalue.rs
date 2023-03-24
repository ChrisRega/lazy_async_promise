use std::future::Future;
use std::mem;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{BoxedSendError, DirectCacheAccess, FutureResult};

/// # A promise which can be easily created and stored.
/// ## Introduction
/// Will spawn a task to resolve the future immediately. No possibility to read out intermediate values or communicate progress.
/// One can use `Option<ImmediateValuePromise<T>>` inside state structs to make this class somewhat lazy.
/// That may be an option if you don't need any progress indication or intermediate values.
/// After the calculation is done, `ImmediateValuePromise<T>` can be read out without mutability requirement using
/// [`ImmediateValuePromise::get_state`] which also yields an [`ImmediateValueState`] but without requiring `mut`.
/// Another useful feature after calculation is finished,
/// is that you can use [`ImmediateValuePromise::poll_state_mut`] to get a mutable [`ImmediateValueState`]
/// which allows you to take ownership of inner values with [`ImmediateValueState::take_value`] or get a mutable reference
/// to the inner via [`ImmediateValueState::get_value_mut`].
/// ## Examples
/// ### Basic usage
/// ```rust, no_run
/// use std::fs::File;
/// use std::thread;
/// use std::time::Duration;
/// use lazy_async_promise::{ImmediateValuePromise, ImmediateValueState};
/// let mut oneshot_val = ImmediateValuePromise::new(async {
///     tokio::time::sleep(Duration::from_millis(50)).await;
///     let test_error_handling = false;
///     if test_error_handling {
///       // We can use the ?-operator for most errors in our futures
///       let _file = File::open("I_DONT_EXIST_ERROR")?;
///     }
///     // return the value wrapped in Ok for the result here
///     Ok(34)
/// });
/// assert!(matches!(
///     oneshot_val.poll_state(),
///     ImmediateValueState::Updating
/// ));
/// thread::sleep(Duration::from_millis(100));
/// let result = oneshot_val.poll_state();
/// if let ImmediateValueState::Success(val) = result {
///     assert_eq!(*val, 34);
/// } else {
///     unreachable!();
/// }
/// ```
/// ### Modifying inner values or taking ownership
/// ```rust, no_run
/// use std::thread;
/// use std::time::Duration;
/// use lazy_async_promise::{DirectCacheAccess, ImmediateValuePromise, ImmediateValueState};
/// let mut oneshot_val = ImmediateValuePromise::new(async {
///     Ok(34)
/// });
/// thread::sleep(Duration::from_millis(50));
/// assert!(matches!(
///     oneshot_val.poll_state(),
///     ImmediateValueState::Success(_)
/// ));
/// let result = oneshot_val.poll_state_mut();
/// // we got the value, take a mutable ref
/// if let ImmediateValueState::Success(inner) = result {
///   *inner=32;
/// }
/// else {
///   unreachable!();
/// }
/// assert!(result.get_value_mut().is_some());
/// // take it out
/// let value = result.take_value();
/// assert_eq!(value.unwrap(), 32);
/// ```
/// ### Optional laziness
/// `Option<ImmediateValuePromise>` is a nice way to implement laziness with `get_or_insert`
///  or `get_or_insert_with`. Unfortunately, using these constructs becomes cumbersome.
/// To ease the pain, we blanket-implement [`DirectCacheAccess`] for any [`Option<DirectCacheAccess<T>>`].
///
/// ```rust, no_run
/// use std::thread;
/// use std::time::Duration;
/// use lazy_async_promise::{DirectCacheAccess, ImmediateValuePromise};
/// #[derive(Default)]
/// struct State {
///   promise: Option<ImmediateValuePromise<i32>>
/// }
///
/// let mut state = State::default();
/// let promise_ref = state.promise.get_or_insert_with(|| ImmediateValuePromise::new(async {
///     Ok(34)
/// }));
/// promise_ref.poll_state();
/// thread::sleep(Duration::from_millis(50));
/// //now let's assume we forgot about our lease already and want to get the value again:
/// let value_opt = state.promise.as_ref().unwrap().get_value(); // <- dangerous
/// let value_opt = state.promise.as_ref().and_then(|i| i.get_value()); // <- better, but still ugly
/// let value_opt = state.promise.get_value(); // <- way nicer!
/// assert!(value_opt.is_some());
/// assert_eq!(*value_opt.unwrap(), 34);
/// ```
///
pub struct ImmediateValuePromise<T: Send + 'static> {
    value_arc: Arc<Mutex<Option<FutureResult<T>>>>,
    state: ImmediateValueState<T>,
}

/// The return state of a [`ImmediateValuePromise`], contains the error, the value or that it is still updating
pub enum ImmediateValueState<T> {
    /// future is not yet resolved
    Updating,
    /// future resolved successfully
    Success(T),
    /// resolving the future failed somehow
    Error(BoxedSendError),
    /// value has been taken out
    Empty,
}

impl<T> DirectCacheAccess<T> for ImmediateValueState<T> {
    /// gets a mutable reference to the local cache if existing
    fn get_value_mut(&mut self) -> Option<&mut T> {
        match self {
            ImmediateValueState::Success(payload) => Some(payload),
            _ => None,
        }
    }
    /// Get the value if possible, [`None`] otherwise
    fn get_value(&self) -> Option<&T> {
        if let ImmediateValueState::Success(inner) = self {
            Some(inner)
        } else {
            None
        }
    }

    /// Takes ownership of the inner value if ready, leaving self in state [`ImmediateValueState::Empty`].
    /// Does nothing if we are in any other state.
    fn take_value(&mut self) -> Option<T> {
        if matches!(self, ImmediateValueState::Success(_)) {
            let val = mem::replace(self, ImmediateValueState::Empty);
            return match val {
                ImmediateValueState::Success(inner) => Some(inner),
                _ => None,
            };
        }
        None
    }
}

impl<T: Send + 'static> DirectCacheAccess<T> for ImmediateValuePromise<T> {
    fn get_value_mut(&mut self) -> Option<&mut T> {
        self.state.get_value_mut()
    }
    fn get_value(&self) -> Option<&T> {
        self.state.get_value()
    }
    fn take_value(&mut self) -> Option<T> {
        self.state.take_value()
    }
}

impl<T: Send> ImmediateValuePromise<T> {
    /// Creator, supply a future which returns `Result<T, Box<dyn Error + Send>`. Will be immediately spawned via tokio.
    pub fn new<U: Future<Output=Result<T, BoxedSendError>> + Send + 'static>(updater: U) -> Self {
        let arc = Arc::new(Mutex::new(None));
        let arc_clone = arc.clone();
        tokio::spawn(async move {
            let mut val = arc_clone.lock().await;
            *val = Some(updater.await);
        });
        Self {
            value_arc: arc,
            state: ImmediateValueState::Updating,
        }
    }

    /// Poll the state updating the internal state from the running thread if possible, will return the data or error if ready or updating otherwise.
    pub fn poll_state(&mut self) -> &ImmediateValueState<T> {
        if matches!(self.state, ImmediateValueState::Updating) {
            let value = self.value_arc.try_lock();
            if let Ok(mut guard) = value {
                if let Some(result) = guard.take() {
                    match result {
                        Ok(value) => self.state = ImmediateValueState::Success(value),
                        Err(e) => self.state = ImmediateValueState::Error(e),
                    };
                }
            }
        }
        &self.state
    }

    /// Poll the state, return a mutable ref to to the state
    pub fn poll_state_mut(&mut self) -> &mut ImmediateValueState<T> {
        let _ = self.poll_state();
        &mut self.state
    }

    /// Get the current state without pulling. No mutability required
    pub fn get_state(&self) -> &ImmediateValueState<T> {
        &self.state
    }
}

#[cfg(test)]
mod test {
    use std::fs::File;
    use std::time::Duration;

    use tokio::runtime::Runtime;

    use crate::DirectCacheAccess;
    use crate::immediatevalue::{ImmediateValuePromise, ImmediateValueState};

    #[test]
    fn default() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(34)
            });
            assert!(matches!(
                oneshot_val.poll_state(),
                ImmediateValueState::Updating
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;
            let result = oneshot_val.poll_state();
            if let ImmediateValueState::Success(val) = result {
                assert_eq!(*val, 34);
                return;
            }
            unreachable!();
        });
    }

    #[test]
    fn error() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async {
                let some_result = File::open("DOES_NOT_EXIST");
                some_result?;
                Ok("bla".to_string())
            });
            assert!(matches!(
                oneshot_val.poll_state(),
                ImmediateValueState::Updating
            ));
            tokio::time::sleep(Duration::from_millis(50)).await;
            let result = oneshot_val.poll_state();
            if let ImmediateValueState::Error(e) = result {
                let _ = format!("{}", **e);
                return;
            }
            unreachable!();
        });
    }

    #[test]
    fn get_state() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async { Ok("bla".to_string()) });
            // get value does not trigger any polling
            let state = oneshot_val.get_state();
            assert!(matches!(state, ImmediateValueState::Updating));
            tokio::time::sleep(Duration::from_millis(50)).await;
            let state = oneshot_val.get_state();
            assert!(matches!(state, ImmediateValueState::Updating));

            let polled = oneshot_val.poll_state();
            assert_eq!(polled.get_value().unwrap(), "bla");
        });
    }

    #[test]
    fn get_mut_take_value() {
        Runtime::new().unwrap().block_on(async {
            let mut oneshot_val = ImmediateValuePromise::new(async { Ok("bla".to_string()) });
            tokio::time::sleep(Duration::from_millis(50)).await;
            {
                // get value does not trigger any polling
                let result = oneshot_val.poll_state_mut();
                // we got the value
                if let Some(inner) = result.get_value_mut() {
                    assert_eq!(inner, "bla");
                    // write back
                    *inner = "changed".to_string();
                } else {
                    unreachable!();
                }
                let result = oneshot_val.poll_state_mut();
                // take it out, should be changed and owned
                let value = result.take_value();
                assert_eq!(value.unwrap().as_str(), "changed");
                assert!(matches!(result, ImmediateValueState::Empty));
            }
            // afterwards we are empty on get and poll
            assert!(matches!(
                oneshot_val.get_state(),
                ImmediateValueState::Empty
            ));
            assert!(matches!(
                oneshot_val.poll_state(),
                ImmediateValueState::Empty
            ));
        });
    }

    #[test]
    fn option_laziness() {
        use crate::*;
        Runtime::new().unwrap().block_on(async {
            let mut option = Some(ImmediateValuePromise::new(async { Ok("bla".to_string()) }));
            tokio::time::sleep(Duration::from_millis(50)).await;
            option.as_mut().unwrap().poll_state();
            let _inner = option.get_value();
            let _inner_mut = option.get_value_mut();
            let inner_owned = option.take_value().unwrap();
            assert_eq!(inner_owned, "bla");
            // after value is taken, we can't borrow it again
            assert!(option.get_value().is_none());
            assert!(option.get_value_mut().is_none());
            assert!(option.take_value().is_none());
        });
    }
}
