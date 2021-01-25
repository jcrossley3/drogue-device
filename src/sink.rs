use crate::actor::Actor;
use crate::handler::{Completion, NotificationHandler};
use heapless::{ArrayLength, Vec};

pub trait Message: 'static + Clone {}

pub trait Sink<M> {
    fn notify(&self, message: M);
}

pub struct MultiSink<M, N>
where
    M: Message,
    N: ArrayLength<&'static dyn Sink<M>>,
{
    subscribers: Vec<&'static dyn Sink<M>, N>,
}

impl<M, N> MultiSink<M, N>
where
    M: Message,
    N: ArrayLength<&'static dyn Sink<M>>,
{
    pub fn new() -> Self {
        MultiSink {
            subscribers: Vec::<_, N>::new(),
        }
    }

    pub fn add(&mut self, subscriber: &'static dyn Sink<M>) {
        self.subscribers.push(subscriber);
    }

    pub fn len(&self) -> usize {
        self.subscribers.len()
    }

    pub fn send(&self, message: M) {
        for sub in self.subscribers.iter() {
            sub.notify(message.clone());
        }
    }
}

impl<M, N> Sink<M> for MultiSink<M, N>
where
    M: Message,
    N: ArrayLength<&'static dyn Sink<M>>,
{
    fn notify(&self, message: M) {
        for sub in self.subscribers.iter() {
            sub.notify(message.clone());
        }
    }
}

pub struct AddSink<M: Message>(&'static dyn Sink<M>);

impl<M: Message> AddSink<M> {
    pub fn new(s: &'static dyn Sink<M>) -> Self {
        AddSink(s)
    }
}

impl<M: Message, N: ArrayLength<&'static dyn Sink<M>>> NotificationHandler<AddSink<M>>
    for MultiSink<M, N>
{
    fn on_notification(&'static mut self, m: AddSink<M>) -> Completion {
        self.add(m.0);
        Completion::immediate()
    }
}

impl<M: Message, N: ArrayLength<&'static dyn Sink<M>>> Actor for MultiSink<M, N> {
    type Event = M;
}

impl Message for () {}
