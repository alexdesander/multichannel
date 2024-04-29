//! # Multichannel
//! A mpmc priority multi channel with dynamic channel registration and freezing.
//! 
//! ## Features
//! - Dynamic channel creation and removal
//! - Priority based message selection
//! - Weighted message selection
//! - Channel freezing
//! - Bounded and unbounded channels
//! - Thread safe
//! - No unsafe code
//! - Multi producer and multi consumer
//! 
//! ## Performance
//! The amount of functionality the DynMultiReceiver provides comes at a cost. Due to the freezing feature,
//! every receive() call has a worst case of O(n) complexity, where n is the amount of channels. This is because
//! the DynMultiReceiver has to iterate over all channels to find the highest priority channel with a message THAT IS NOT FROZEN.
//! Otherwise using a heap would be a good idea, but the freezing feature makes this impossible.
//! 
//! So if you have a huge amount of channels and you are not using the freezing feature, you might want to consider using a different
//! implementation. For most use cases, the performance should be good enough.
//! 
//! If you can implement your logic using only basic channels, you should do that. This implementation is meant for cases where
//! you need more advanced features.
//! 
//! ## Hello World
//! ```
//!  use multichannel::DynMultiReceiver;
//!
//! #[derive(Debug)]
//! enum Msg {
//!     Shutdown,
//!     IntegerData(i32),
//!     FloatingData(f32),
//! }
//!
//! #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
//! enum Priority {
//!     High,
//!     Low,
//! }
//!
//! fn main() {
//!     let mrx = DynMultiReceiver::<Msg, Priority>::new();
//!     // Create an unfrozen channel with high priority, a dummy weight and capacity of 1
//!     let shutdown_sender = mrx.new_channel(Priority::High, 1, false, Some(1));
//!
//!     // Create two channels with low priority
//!     // int_sender has a weight of 33 and float_sender has a weight of 66
//!     // meaning that float_sender will be twice as likely to be selected
//!     // when calling receive() on mrx and no higher priority channel has a msg
//!     let int_sender = mrx.new_channel(Priority::Low, 33, false, None);
//!     let float_sender = mrx.new_channel(Priority::Low, 66, false, None);
//!
//!     // Send some messages
//!     int_sender.send(Msg::IntegerData(33)).unwrap();
//!     int_sender.send(Msg::IntegerData(4031)).unwrap();
//!     float_sender.send(Msg::FloatingData(3.14)).unwrap();
//!     int_sender.send(Msg::IntegerData(2)).unwrap();
//!     float_sender.send(Msg::FloatingData(10.0)).unwrap();
//!     float_sender.send(Msg::FloatingData(0.0)).unwrap();
//!
//!     // Receive some messages
//!     for _ in 0..4 {
//!         println!("{:?}", mrx.receive());
//!     }
//!
//!     // Send a shutdown message
//!     shutdown_sender.send(Msg::Shutdown).unwrap();
//!
//!     // There are still messages left in the int channel and float channel,
//!     // but the shutdown message will be received first, as it has higher priority
//!     match mrx.receive() {
//!         Msg::Shutdown => println!("Received shutdown message"),
//!         _ => unreachable!("Expected a shutdown message"),
//!     }
//! }
//! ```



use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex, RwLock,
};

use ahash::{HashMap, HashSet};
use rand::distributions::{Distribution, WeightedIndex};
use smallvec::SmallVec;
use thiserror::Error;

mod tests;

pub trait Priority: Ord {}
impl<P: Ord> Priority for P {}

struct DynState<T, P: Priority> {
    next_id: u32,
    lookup: HashMap<u32, (usize, usize)>, // (group_idx, inner_idx)
    groups: Vec<PriorityGroup<T, P>>,
}

impl<T, P: Priority> DynState<T, P> {
    fn new() -> Self {
        Self {
            next_id: 0,
            lookup: HashMap::default(),
            groups: Vec::new(),
        }
    }

    pub fn add_receiver(&mut self, priority: P, receiver: DynReceiver<T>) {
        debug_assert!(self.lookup.contains_key(&receiver.id) == false);
        let channel_id = receiver.id;
        let group_idx;
        let inner_idx;
        match self.groups.binary_search_by(|g| g.priority.cmp(&priority)) {
            Ok(idx) => {
                self.groups[idx].receivers.push(receiver);
                group_idx = idx;
                inner_idx = self.groups[idx].receivers.len() - 1;
            }
            Err(idx) => {
                let mut group = PriorityGroup::new(priority);
                group.receivers.push(receiver);
                self.groups.insert(idx, group);
                group_idx = idx;
                inner_idx = 0;
                // Adjust lookup
                for group in &self.groups[idx + 1..] {
                    for receiver in &group.receivers {
                        let (group_idx, _) = self.lookup.get_mut(&receiver.id).unwrap();
                        *group_idx += 1;
                    }
                }
            }
        }
        self.lookup.insert(channel_id, (group_idx, inner_idx));
    }

    pub fn remove_receiver(&mut self, id: u32) {
        let (group_idx, inner_idx) = self.lookup.remove(&id).unwrap();
        self.groups[group_idx].receivers.remove(inner_idx);
        // Adjust lookup
        for receiver in &self.groups[group_idx].receivers[inner_idx..] {
            let (_, inner_idx) = self.lookup.get_mut(&receiver.id).unwrap();
            *inner_idx -= 1;
        }
        // Remove group if empty
        if self.groups[group_idx].receivers.is_empty() {
            self.groups.remove(group_idx);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    fn set_frozen(&mut self, id: u32, frozen: bool) {
        let (group_idx, inner_idx) = self.lookup.get(&id).unwrap();
        self.groups[*group_idx].receivers[*inner_idx].frozen = frozen;
    }
}

struct PriorityGroup<T, P: Priority> {
    priority: P,
    receivers: Vec<DynReceiver<T>>,
}

impl<T, P: Priority> PriorityGroup<T, P> {
    fn new(priority: P) -> Self {
        Self {
            priority,
            receivers: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum SendError {
    #[error("The channel receiver is disconnected")]
    Disconnected,
}

pub struct DynSender<T, P: Priority> {
    id: u32,
    count_multireceivers: Arc<AtomicUsize>,
    condvar: Arc<(Mutex<usize>, Condvar)>,
    state: Arc<RwLock<DynState<T, P>>>,
    inner: crossbeam_channel::Sender<T>,
}

impl<T, P: Priority> DynSender<T, P> {
    pub fn id(&self) -> u32 {
        self.id
    }

    fn wake_receiver(&self) {
        let (lock, condvar) = &*self.condvar;
        {
            let mut count = lock.lock().unwrap();
            *count += 1;
        }
        condvar.notify_one();
    }

    pub fn send(&self, value: T) -> Result<(), SendError> {
        if self.count_multireceivers.load(Ordering::Relaxed) == 0 {
            return Err(SendError::Disconnected);
        }
        if self.inner.capacity() == Some(0) {
            self.wake_receiver();
        }
        if self.inner.send(value).is_err() {
            return Err(SendError::Disconnected);
        }
        if !(self.inner.capacity() == Some(0)) {
            self.wake_receiver();
        }
        Ok(())
    }

    pub fn set_frozen(&self, frozen: bool) {
        let mut state = self.state.write().unwrap();
        state.set_frozen(self.id, frozen);
    }
}

struct DynReceiver<T> {
    id: u32,
    weight: u32,
    frozen: bool,
    inner: crossbeam_channel::Receiver<T>,
}

pub struct DynMultiReceiver<T, P: Priority> {
    amount_multireceivers: Arc<AtomicUsize>,
    cleanup: Arc<(AtomicBool, Mutex<HashSet<u32>>)>,
    condvar: Arc<(Mutex<usize>, Condvar)>,
    state: Arc<RwLock<DynState<T, P>>>,
}

impl<T, P: Priority> Clone for DynMultiReceiver<T, P> {
    fn clone(&self) -> Self {
        self.amount_multireceivers.fetch_add(1, Ordering::Relaxed);
        Self {
            amount_multireceivers: self.amount_multireceivers.clone(),
            cleanup: self.cleanup.clone(),
            condvar: self.condvar.clone(),
            state: self.state.clone(),
        }
    }
}

impl<T, P: Priority> Drop for DynMultiReceiver<T, P> {
    fn drop(&mut self) {
        self.amount_multireceivers.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<T, P: Priority> DynMultiReceiver<T, P> {
    pub fn new() -> Self {
        Self {
            amount_multireceivers: Arc::new(AtomicUsize::new(1)),
            cleanup: Arc::new((AtomicBool::new(false), Mutex::new(HashSet::default()))),
            condvar: Arc::new((Mutex::new(0), Condvar::new())),
            state: Arc::new(RwLock::new(DynState::new())),
        }
    }

    /// Create a new channel with the given priority, weight, frozen state and optional bounds.
    /// 
    /// The weight is used to determine the probability of the channel being selected when calling receive()
    /// on the DynMultiReceiver. The weight is relative to the weights of other channels in the same priority group.
    /// The weight must be greater than 0.
    /// 
    /// The frozen state determines if the channel is considered when calling receive() on the DynMultiReceiver.
    /// If the channel is frozen, it will not be considered, even if it has a message.
    /// 
    /// The bounds parameter is used to create a bounded channel. If None is passed, an unbounded channel is created.
    /// If Some(bounds) is passed, a bounded channel with the given bounds is created.
    pub fn new_channel(
        &self,
        priority: P,
        weight: u32,
        frozen: bool,
        bounds: Option<usize>,
    ) -> DynSender<T, P> {
        assert!(weight > 0, "Weight must be greater than 0");
        let (sender, receiver) = match bounds {
            Some(bounds) => crossbeam_channel::bounded(bounds),
            None => crossbeam_channel::unbounded(),
        };
        let id;
        {
            let mut state = self.state.write().unwrap();
            id = state.next_id;
            state.next_id += 1;
            let receiver = DynReceiver {
                id,
                weight,
                frozen,
                inner: receiver,
            };
            state.add_receiver(priority, receiver);
        }
        DynSender {
            id,
            count_multireceivers: self.amount_multireceivers.clone(),
            condvar: self.condvar.clone(),
            state: self.state.clone(),
            inner: sender,
        }
    }

    pub fn remove_channel_by_id(&self, id: u32) {
        self.state.write().unwrap().remove_receiver(id);
    }

    pub fn remove_channel(&self, sender: &DynSender<T, P>) {
        self.remove_channel_by_id(sender.id);
    }

    pub fn receive(&self) -> T {
        if self.cleanup.0.fetch_and(false, Ordering::Relaxed) {
            let mut state = self.state.write().unwrap();
            let mut to_clean = self.cleanup.1.lock().unwrap();
            for id in to_clean.drain() {
                state.remove_receiver(id);
            }
        }

        let (lock, condvar) = &*self.condvar;
        {
            let mut count = lock.lock().unwrap();
            while *count == 0 {
                count = condvar.wait(count).unwrap();
            }
            *count -= 1;
        }
        let state = self.state.read().unwrap();

        // Find the highest priority group with a receiver that has a message
        // TODO: Handle 0 capacity channels
        let mut candidate_weights = SmallVec::<[u32; 8]>::new();
        let mut candidate_indices = SmallVec::<[usize; 8]>::new();
        loop {
            for group in &state.groups {
                candidate_indices.clear();
                candidate_weights.clear();
                for i in 0..group.receivers.len() {
                    let receiver = &group.receivers[i];
                    if (receiver.inner.len() > 0 && !receiver.frozen)
                        || (receiver.inner.capacity() == Some(0))
                    {
                        candidate_indices.push(i);
                        candidate_weights.push(receiver.weight);
                    }
                }
                while !candidate_indices.is_empty() {
                    let dist = WeightedIndex::new(&candidate_weights).unwrap();
                    let candidate_index = dist.sample(&mut rand::thread_rng());
                    let idx = candidate_indices[candidate_index];
                    match group.receivers[idx].inner.try_recv() {
                        Ok(value) => return value,
                        Err(crossbeam_channel::TryRecvError::Empty) => {
                            candidate_indices.remove(candidate_index);
                            candidate_weights.remove(candidate_index);
                            continue;
                        }
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            self.cleanup
                                .1
                                .lock()
                                .unwrap()
                                .insert(group.receivers[idx].id);
                            self.cleanup.0.store(true, Ordering::Relaxed);
                            candidate_indices.remove(candidate_index);
                            candidate_weights.remove(candidate_index);
                            continue;
                        }
                    };
                }
            }
        }
    }

    pub fn no_channels(&self) -> bool {
        self.state.read().unwrap().is_empty()
    }
}
