//! ===== THIS IS OUR "NODE LIBRARY" =====
//! You'll use this library like this
//! ```rust
//! # extern crate js_event_loop;
//! use js_event_loop::Runtime;
//! fn main() {
//!    let mut rt = Runtime::new();
//!    rt.run(|| {
//!        println!("Hello world!");
//!    });
//! }
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::io;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod minimio;
pub mod modules;

pub static mut RUNTIME: usize = 0;

struct Event {
    task: Box<Fn() -> Js + Send + 'static>,
    callback_id: usize,
}

pub enum EventKind {
    FileRead,
    Encrypt,
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use EventKind::*;
        match self {
            FileRead => write!(f, "File read"),
            Encrypt => write!(f, "Encrypt"),
        }
    }
}

#[derive(Debug)]
pub enum Js {
    Undefined,
    String(String),
    Int(usize),
}

impl Js {
    /// Convenience method since we know the types
    pub fn into_string(self) -> Option<String> {
        match self {
            Js::String(s) => Some(s),
            _ => None,
        }
    }

    /// Convenience method since we know the types
    pub fn into_int(self) -> Option<usize> {
        match self {
            Js::Int(n) => Some(n),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct NodeThread {
    handle: JoinHandle<()>,
    sender: Sender<Event>,
}

enum Workload {
    High,
    Normal,
}

pub struct Runtime {
    thread_pool: Box<[NodeThread]>,
    work_queue: VecDeque<Event>,
    available: Vec<usize>,
    callback_queue: HashMap<usize, Box<FnOnce(Js)>>,
    next_tick_callbacks: Vec<(usize, Js)>,
    identity_token: usize,
    pending_events: usize,
    threadp_reciever: Receiver<(usize, usize, Js)>,
    epoll_reciever: Receiver<usize>,
    epoll_queue: i32,
    epoll_pending: usize,
    epoll_starter: Sender<usize>,
    epoll_event_cb_map: HashMap<i64, usize>,
    timers: BTreeMap<Instant, usize>,
}

impl Runtime {
    pub fn new() -> Self {
        // ===== THE REGULAR THREADPOOL =====
        let (threadp_sender, threadp_reciever) = channel::<(usize, usize, Js)>();
        let mut threads = Vec::with_capacity(4);
        for i in 0..4 {
            let (evt_sender, evt_reciever) = channel::<Event>();
            let threadp_sender = threadp_sender.clone();
            let handle = thread::Builder::new()
                .name(format!("pool{}", i))
                .spawn(move || {
                    while let Ok(event) = evt_reciever.recv() {
                        let res = (event.task)();
                        threadp_sender.send((i, event.callback_id, res)).unwrap();
                    }
                })
                .expect("Couldn't initialize thread pool.");

            let node_thread = NodeThread {
                handle,
                sender: evt_sender,
            };

            threads.push(node_thread);
        }

        // ===== EPOLL THREAD =====
        // Only wakes up when there is a task ready
        let (epoll_sender, epoll_reciever) = channel::<usize>();
        let (epoll_start_sender, epoll_start_reciever) = channel::<usize>();
        let queue = minimio::queue().expect("Error creating epoll queue");
        thread::Builder::new()
            .name("epoll".to_string())
            .spawn(move || loop {
                // let mut changes: Vec<minimio::Event> = (0..MAX_EPOLL_EVENTS)
                // .map(|_| minimio::Event::default())
                // .collect();

                let mut changes = vec![];
                while let Ok(current_event_count) = epoll_start_reciever.recv() {
                    // We increase the size to hold the current number of events but
                    // it will always grow to the largest load and stay there. We could implement
                    // a resize strategy to not hold on to more memory than we need but won't do
                    // that here
                    if changes.len() < current_event_count {
                        let missing = current_event_count - changes.len();
                        (0..missing).for_each(|_| changes.push(minimio::Event::default()));
                    }
                    match minimio::poll(queue, changes.as_mut_slice(), 0, None) {
                        Ok(v) if v > 0 => {
                            for i in 0..v {
                                let event = changes.get_mut(i).expect("No events in event list.");
                                epoll_sender.send(event.ident as usize).unwrap();
                            }
                        }
                        Err(e) => panic!("{:?}", e),
                        _ => (),
                    }
                }
            })
            .expect("Error creating epoll thread");

        Runtime {
            thread_pool: threads.into_boxed_slice(),
            work_queue: VecDeque::new(),
            available: (0..4).collect(),
            callback_queue: HashMap::new(),
            next_tick_callbacks: vec![],
            identity_token: 0,
            pending_events: 0,
            threadp_reciever,
            epoll_reciever,
            epoll_queue: queue,
            epoll_pending: 0,
            epoll_starter: epoll_start_sender,
            epoll_event_cb_map: HashMap::new(),
            timers: BTreeMap::new(),
        }
    }

    /// This is the event loop. There are several things we could do here to make it a better implementation
    /// One is to set a max backlog of callbacks to execute in a single tick, so we don't starve the
    /// threadpool or file handlers.
    /// Another is to dynamically decide if/and how long the thread could be allowed to be parked for
    /// example by looking at the backlog of events, and if there is any backlog disable it.
    /// Some of our Vec's will only grow, and not resize, so if we have a period of very high load,
    /// the memory will stay higher than we need until a restart. This could be dealt with or a different
    /// data structure could be used.
    pub fn run(&mut self, f: impl Fn()) {
        let rt_ptr: *mut Runtime = self;
        unsafe { RUNTIME = rt_ptr as usize };

        let mut timers_to_remove = vec![]; // avoid allocating on every loop
        let mut ticks = 0; // just for us priting out

        // First we run our "main" function
        f();

        // ===== EVENT LOOP =====
        while self.pending_events > 0 {
            ticks += 1;

            // ===== TIMERS =====

            self.timers
                .range(..=Instant::now())
                .for_each(|(k, _)| timers_to_remove.push(*k));

            while let Some(key) = timers_to_remove.pop() {
                let callback_id = self.timers.remove(&key).unwrap();
                self.next_tick_callbacks.push((callback_id, Js::Undefined));
            }

            // ===== CALLBACKS =====

            while let Some((callback_id, data)) = self.next_tick_callbacks.pop() {
                let cb = self.callback_queue.remove(&callback_id).unwrap();
                cb(data);
                self.pending_events -= 1;
            }

            // ===== IDLE/PREPARE =====

            // we won't use this

            // ===== POLL =====

            // First poll any epoll/kqueue
            while let Ok(event_id) = self.epoll_reciever.try_recv() {
                let id = self
                    .epoll_event_cb_map
                    .get(&(event_id as i64))
                    .expect("Event not in event map.");
                let callback_id = *id;
                self.epoll_event_cb_map.remove(&(event_id as i64));

                self.next_tick_callbacks.push((callback_id, Js::Undefined));
                self.epoll_pending -= 1;
            }

            // then check if there is any results from the threadpool
            while let Ok((thread_id, callback_id, data)) = self.threadp_reciever.try_recv() {
                self.next_tick_callbacks.push((callback_id, data));
                // If we have pending tasks we hijack the thread here
                if !self.work_queue.is_empty() {
                    let event = self.work_queue.pop_front().unwrap();
                    self.thread_pool[thread_id].sender.send(event).unwrap();
                    self.pending_events += 1;
                } else {
                    self.available.push(thread_id);
                }
            }

            // ===== CHECK =====
            // an set immidiate function could be added pretty easily but we won't do that here

            // ===== CLOSE CALLBACKS ======
            // Release resources, we won't do that here, it's just another "hook" for our "extensions"
            // to use. We release in every callback instead

            // Let the OS have a time slice of our thread so we don't busy loop
            match self.current_workload() {
                Workload::High => (),
                Workload::Normal => thread::park_timeout(Duration::from_millis(1)),
            }
        }
    }

    /// If we have any pending callbacks to execute we have to much to do to 
    /// give the OS a time slice
    fn current_workload(&self) -> Workload {
        if !self.next_tick_callbacks.is_empty() {
            Workload::High 
        } else {
            Workload::Normal
        }
    }

    fn schedule(&mut self) -> Option<usize> {
        self.available.pop()
    }

    /// If we hit max we just wrap around
    fn generate_identity(&mut self) -> usize {
        self.identity_token = self.identity_token.wrapping_add(1);
        self.identity_token
    }

    /// Adds a callback to the queue and returns the key
    fn add_callback(&mut self, cb: impl FnOnce(Js) + 'static) -> usize {
        // this is the happy path
        let ident = self.generate_identity();
        let boxed_cb = Box::new(cb);
        let taken = self.callback_queue.contains_key(&ident);

        // if there is a collision or the identity is already there we loop until we find a new one
        // we don't cover the case where there are `usize::MAX` number of callbacks waiting since
        // that if we're fast and queue a new event every nanosecond that will still take 585.5 years
        // to do on a 64 bit system.
        if !taken {
            self.callback_queue.insert(ident, boxed_cb);
            ident
        } else {
            loop {
                let possible_ident = self.generate_identity();
                if self.callback_queue.contains_key(&possible_ident) {
                    self.callback_queue.insert(possible_ident, boxed_cb);
                    break possible_ident;
                }
            }
        }
    }

    pub fn register_io(&mut self, mut event: minimio::Event, cb: impl FnOnce(Js) + 'static) {
        let cb_id = self.add_callback(cb) as i64;

        // if no ident is set, set it equal to cb_id + 1 000 000
        if event.ident == 0 {
            event.ident = cb_id as u64 + 1_000_000;
        }

        self.epoll_event_cb_map
            .insert(event.ident as i64, cb_id as usize);

        minimio::add_event(self.epoll_queue, &[event.clone()], 0)
            .expect("Error adding event to queue.");

        self.pending_events += 1;
        self.epoll_pending += 1;
        self.epoll_starter
            .send(self.epoll_pending)
            .expect("Sending to epoll_starter.");
    }

    pub fn register_work(
        &mut self,
        task: impl Fn() -> Js + Send + 'static,
        cb: impl FnOnce(Js) + 'static,
    ) {
        let callback_id = self.add_callback(cb);

        let event = Event {
            task: Box::new(task),
            callback_id,
        };

        // we are not going to implement a real scheduler here, just a LIFO queue
        let available = self.schedule();
        match available {
            Some(available_id) => {
                self.thread_pool[available_id].sender.send(event).unwrap();
                self.pending_events += 1;
            },
            None => self.work_queue.push_back(event),
        };
        
    }

    fn set_timeout(&mut self, ms: u64, cb: impl Fn(Js) + 'static) {
        // Is it theoretically possible to get two equal instants? If so we will have a bug...
        let now = Instant::now();
        let cb_id = self.add_callback(cb);
        let timeout = now + Duration::from_millis(ms);
        self.timers.insert(timeout, cb_id);
        self.pending_events += 1;
    }
}

pub fn set_timeout(ms: u64, cb: impl Fn(Js) + 'static) {
    let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
    rt.set_timeout(ms, cb);
}


