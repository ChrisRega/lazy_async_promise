use crate::{BoxedSendError, ImmediateValuePromise, ImmediateValueState};
use crate::{DirectCacheAccess, Progress};
use std::borrow::Cow;
use std::future::Future;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;

pub struct Status<M> {
    time: Instant,
    progress: Progress,
    message: M,
}

pub type StringStatus = Status<Cow<'static, str>>;

pub struct ProgressTrackedImValProm<T: Send, M> {
    promise: ImmediateValuePromise<T>,
    status: Vec<Status<M>>,
    receiver: Receiver<Status<M>>,
}

impl<T: Send + 'static, M> ProgressTrackedImValProm<T, M> {
    pub fn new(
        creator: impl FnOnce(Sender<Status<M>>) -> ImmediateValuePromise<T>,
        buffer: usize,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(buffer);
        ProgressTrackedImValProm {
            receiver,
            status: Vec::new(),
            promise: creator(sender),
        }
    }

    pub fn finished(&self) -> bool {
        self.promise.get_value().is_some()
    }

    pub fn poll_state(&mut self) -> &ImmediateValueState<T> {
        while let Ok(msg) = self.receiver.try_recv() {
            self.status.push(msg);
        }
        self.promise.poll_state()
    }

    pub fn progress(&self) -> Progress {
        self.status
            .last()
            .map(|p| p.progress)
            .unwrap_or(Progress::default())
    }
}

impl<T: Send + 'static, M> DirectCacheAccess<T> for ProgressTrackedImValProm<T, M> {
    fn get_value_mut(&mut self) -> Option<&mut T> {
        self.promise.get_value_mut()
    }
    fn get_value(&self) -> Option<&T> {
        self.promise.get_value()
    }
    fn take_value(&mut self) -> Option<T> {
        self.promise.take_value()
    }
}
