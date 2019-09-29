/// Think of this function as the javascript program you have written
fn javascript() {
    print("First call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        print(format!("First count: {} characters.", len));

        print(r#"I want to create a "magic" number based on the text."#);
        Crypto::encrypt(text.len(), |result| {
            let n = result.into_int().unwrap();
            print(format!(r#""Encrypted" number is: {}"#, n));
        })
    });

    print("Registering immediate timeout 1");
    set_timeout(0, |_res| {
        print("Immediate1 timed out");
    });
    print("Registering immediate timeout 2");
    set_timeout(0, |_res| {
        print("Immediate2 timed out");
    });

    // let's read the file again and display the text
    print("Second call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        print(format!("Second count: {} characters.", len));

        // aaand one more time but not in parallel.
        print("Third call to read test.txt");
        Fs::read("test.txt", |result| {
            let text = result.into_string().unwrap();
            print_content(&text, "file read");
        });
    });

    print("Registering a 3000 and a 500 ms timeout");
    set_timeout(3000, |_res| {
        print("3000ms timer timed out");
        set_timeout(500, |_res| {
            print("500ms timer(nested) timed out");
        });
    });

    print("Registering a 1000 ms timeout");
    set_timeout(1000, |_res| {
        print("SETTIMEOUT");
    });

    // `http_get_slow` let's us define a latency we want to simulate
    print("Registering http get request to google.com");
    Http::http_get_slow("http//www.google.com", 2000, |result| {
        let result = result.into_string().unwrap();
        print_content(result.trim(), "web call");
    });   
    // `http_get_slow` let's us define a latency we want to simulate
    print("Registering http get request to mozilla.org");
    Http::http_get_slow("http//www.mozilla.org", 1000, |result| {
        let result = result.into_string().unwrap();
        print_content(result.trim(), "web call");
    });
}

fn main() {
    let rt = Runtime::new();
    rt.run(javascript);
}

// ===== THIS IS OUR "NODE LIBRARY" =====
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use minimio;

static mut RUNTIME: *mut Runtime = std::ptr::null_mut();

struct Event {
    task: Box<dyn Fn() -> Js + Send + 'static>,
    callback_id: usize,
    kind: ThreadPoolEventKind,
}

impl Event {
    fn close() -> Self {
        Event {
            task: Box::new(|| Js::Undefined),
            callback_id: 0,
            kind: ThreadPoolEventKind::Close,
        }
    }
}

pub enum ThreadPoolEventKind {
    FileRead,
    Encrypt,
    Close,
}

impl fmt::Display for ThreadPoolEventKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ThreadPoolEventKind::*;
        match self {
            FileRead => write!(f, "File read"),
            Encrypt => write!(f, "Encrypt"),
            Close => write!(f, "Close"),
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
    fn into_string(self) -> Option<String> {
        match self {
            Js::String(s) => Some(s),
            _ => None,
        }
    }

    /// Convenience method since we know the types
    fn into_int(self) -> Option<usize> {
        match self {
            Js::Int(n) => Some(n),
            _ => None,
        }
    }
}

/// NodeTheread represents a thread in our threadpool. Each event has a Joinhandle
/// and a transmitter part of a channel which is used to inform our main loop
/// about what events has occurred.
#[derive(Debug)]
struct NodeThread {
    pub(crate) handle: JoinHandle<()>,
    sender: Sender<Event>,
}

pub struct Runtime {
    /// Available threads for the threadpool
    available_threads: Vec<usize>,
    /// Callbacks scheduled to run
    callbacks_to_run: Vec<(usize, Js)>,
    /// All registered callbacks
    callback_queue: HashMap<usize, Box<dyn FnOnce(Js)>>,
    /// Maps an epoll event to a callback
    epoll_event_cb_map: HashMap<i64, usize>,
    /// Number of pending epoll events, only used by us to print for this example
    epoll_pending_events: usize,
    /// Our event registrator which registers interest in events with the OS
    epoll_registrator: minimio::Registrator,
    // The handle to our epoll thread
    epoll_thread: thread::JoinHandle<()>,
    /// None = infinite, Some(n) = timeout in n ms, Some(0) = immidiate
    epoll_timeout: Arc<Mutex<Option<i32>>>,
    /// Channel used by both our threadpool and our epoll thread to send events
    /// to the main loop
    event_reciever: Receiver<PollEvent>,
    /// Creates an unique identity for our callbacks
    identity_token: usize,
    /// The number of events pending. When this is zero, we're done
    pending_events: usize,
    /// Handles to our threads in the threadpool
    thread_pool: Vec<NodeThread>,
    /// Holds all our timers, and an Id for the callback to run once they expire
    timers: BTreeMap<Instant, usize>,
    /// A struct to temporarely hold timers to remove. We let Runtinme have
    /// ownership so we can reuse the same memory
    timers_to_remove: Vec<Instant>,
}

/// Describes the three main events our epoll-eventloop handles
enum PollEvent {
    /// An event from the `threadpool` with a tuple containing the `thread id`,
    /// the `callback_id` and the data which the we expect to process in our
    /// callback 
    Threadpool((usize, usize, Js)),
    /// An event from the epoll-based eventloop holding the `event_id` for the
    /// event
    Epoll(usize),
    Timeout,
}

impl Runtime {
    pub fn new() -> Self {
        // ===== THE REGULAR THREADPOOL =====
        let (event_sender, event_reciever) = channel::<PollEvent>();
        let mut threads = Vec::with_capacity(4);
        for i in 0..4 {
            let (evt_sender, evt_reciever) = channel::<Event>();
            let event_sender = event_sender.clone();
            let handle = thread::Builder::new()
                .name(format!("pool{}", i))
                .spawn(move || {
                    while let Ok(event) = evt_reciever.recv() {
                        print(format!("recived a task of type: {}", event.kind));
                        // check if we're closing the loop
                        if let ThreadPoolEventKind::Close = event.kind {
                            break;
                        };

                        let res = (event.task)();
                        print(format!("finished running a task of type: {}.", event.kind));

                        let event = PollEvent::Threadpool((i, event.callback_id, res));
                        event_sender.send(event).expect("threadpool");
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
        let mut poll = minimio::Poll::new().expect("Error creating epoll queue");
        let registrator = poll.registrator();
        let epoll_timeout = Arc::new(Mutex::new(None));
        let epoll_timeout_clone = epoll_timeout.clone();

        let epoll_thread = thread::Builder::new()
            .name("epoll".to_string())
            .spawn(move || {
                let mut events = minimio::Events::with_capacity(1024);
                loop {
                    println!("LOOP");
                    let epoll_timeout_handle = epoll_timeout_clone.lock().unwrap();
                    let timeout = *epoll_timeout_handle;
                    drop(epoll_timeout_handle);

                    match poll.poll(&mut events, timeout) {
                        Ok(v) if v > 0 => {
                            for i in 0..v {
                                let event = events.get_mut(i).expect("No events in event list.");
                                print(format!("epoll event {} is ready", event.id().value()));
                                let event = PollEvent::Epoll(event.id().value() as usize);
                                event_sender.send(event).expect("epoll event");
                            }
                        }
                        Ok(v) if v == 0 => {
                            print("epoll event timeout is ready");
                            event_sender.send(PollEvent::Timeout).expect("epoll timeout");
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {
                            print("recieved event of type: Close");
                            break;
                        }
                        Err(e) => panic!("{:?}", e),
                        _ => (),
                    }
                }
            })
            .expect("Error creating epoll thread");

        Runtime {
            available_threads: (0..4).collect(),
            callbacks_to_run: vec![],
            callback_queue: HashMap::new(),
            epoll_event_cb_map: HashMap::new(),
            epoll_pending_events: 0,
            epoll_registrator: registrator,
            epoll_thread,
            epoll_timeout,
            event_reciever,
            identity_token: 0,
            pending_events: 0,
            thread_pool: threads,
            timers: BTreeMap::new(),
            timers_to_remove: vec![],
        }
    }

    /// This is the event loop. There are several things we could do here to
    /// make it a better implementation. One is to set a max backlog of callbacks
    /// to execute in a single tick, so we don't starve the threadpool or file
    /// handlers. Another is to dynamically decide if/and how long the thread
    /// could be allowed to be parked for example by looking at the backlog of
    /// events, and if there is any backlog disable it. Some of our Vec's will
    /// only grow, and not resize, so if we have a period of very high load, the
    /// memory will stay higher than we need until a restart. This could be
    /// dealt by using a different kind of data structure like a `LinkedList`.
    pub fn run(mut self, f: impl Fn()) {
        let rt_ptr: *mut Runtime = &mut self;
        unsafe { RUNTIME = rt_ptr };
        let mut ticks = 0; // just for us priting out during execution

        // First we run our "main" function
        f();

        // ===== EVENT LOOP =====
        while self.pending_events > 0 {
            ticks += 1;
            // NOT PART OF LOOP, JUST FOR US TO SEE WHAT TICK IS EXCECUTING
            print(format!("===== TICK {} =====", ticks));

            // ===== 2. TIMERS =====
            self.process_expired_timers();

            // ===== 2. CALLBACKS =====
            // Timer callbacks and if for some reason we have postponed callbacks
            // to run on the next tick. Not possible in our implementation though.
            self.run_callbacks();

            // ===== 3. IDLE/PREPARE =====
            // we won't use this

            // ===== 4. POLL =====
            // First we need to check if we have any outstanding events at all
            // and if not we're finished. If not we will wait forever.
            if self.pending_events == 0 {
                break;
            }

            // We want to get the time to the next timeout (if any) and we
            // set the timeout of our epoll wait to the same as the timeout
            // for the next timer. If there is none, we set it to infinite (None)
            let next_timeout = self.get_next_timer();
            println!("NEXT_TIMEOUT: {:?}", next_timeout);
            // We release the lock before we wait
            let mut epoll_timeout_lock = self.epoll_timeout.lock().unwrap();
            *epoll_timeout_lock = next_timeout;
            drop(epoll_timeout_lock);

            // We handle one and one event but multiple events could be returned
            // on the same poll. We won't cover that here though but there are
            // several ways of handling this.
            if let Ok(event) = self.event_reciever.recv() {
                match event {
                    PollEvent::Timeout => (),
                    PollEvent::Threadpool((thread_id, callback_id, data)) => {
                        self.process_threadpool_events(thread_id, callback_id, data);
                    }
                    PollEvent::Epoll(event_id) => {
                        self.process_epoll_events(event_id);
                    }
                }
            }
            self.run_callbacks();

            // ===== 5. CHECK =====
            // an set immidiate function could be added pretty easily but we
            // won't do that here

            // ===== 6. CLOSE CALLBACKS ======
            // Release resources, we won't do that here, but this is typically
            // where sockets etc are closed.
        }

        // We clean up our resources, makes sure all destructors runs.
        for thread in self.thread_pool.into_iter() {
            thread.sender.send(Event::close()).expect("threadpool cleanup");
            thread.handle.join().unwrap();
        }
        self.epoll_registrator.close_loop().unwrap();
        self.epoll_thread.join().unwrap();
        print("FINISHED");
    }

    fn process_expired_timers(&mut self) {
        // Need an intermediate variable to please the borrowchecker
        let timers_to_remove = &mut self.timers_to_remove;

        self.timers
            .range(..=Instant::now())
            .for_each(|(k, _)| timers_to_remove.push(*k));

        while let Some(key) = self.timers_to_remove.pop() {
            let callback_id = self.timers.remove(&key).unwrap();
            self.callbacks_to_run.push((callback_id, Js::Undefined));
        }
    }

    fn get_next_timer(&self) -> Option<i32> {
        self.timers.iter().nth(0).map(|(&instant, _)| {
            let mut time_to_next_timeout = instant - Instant::now();
            if time_to_next_timeout < Duration::new(0, 0) {
                time_to_next_timeout = Duration::new(0, 0);
            }
            time_to_next_timeout.as_millis() as i32
        })
    }

    fn run_callbacks(&mut self) {
        while let Some((callback_id, data)) = self.callbacks_to_run.pop() {
            let cb = self.callback_queue.remove(&callback_id).unwrap();
            cb(data);
            self.pending_events -= 1;
        }
    }

    fn process_epoll_events(&mut self, event_id: usize) {
        let id = self
            .epoll_event_cb_map
            .get(&(event_id as i64))
            .expect("Event not in event map.");
        let callback_id = *id;
        self.epoll_event_cb_map.remove(&(event_id as i64));

        self.callbacks_to_run.push((callback_id, Js::Undefined));
        self.epoll_pending_events -= 1;
    }

    fn process_threadpool_events(&mut self, thread_id: usize, callback_id: usize, data: Js) {
        self.callbacks_to_run.push((callback_id, data));
        self.available_threads.push(thread_id);
    }

    fn get_available_thread(&mut self) -> usize {
        match self.available_threads.pop() {
            Some(thread_id) => thread_id,
            // We would normally return None and the request and not panic!
            None => panic!("Out of threads."),
        }
    }

    /// If we hit max we just wrap around
    fn generate_identity(&mut self) -> usize {
        self.identity_token = self.identity_token.wrapping_add(1);
        self.identity_token
    }

    fn generate_cb_identity(&mut self) -> usize {
        let ident = self.generate_identity();
        let taken = self.callback_queue.contains_key(&ident);

        // if there is a collision or the identity is already there we loop until we find a new one
        // we don't cover the case where there are `usize::MAX` number of callbacks waiting since
        // that if we're fast and queue a new event every nanosecond that will still take 585.5 years
        // to do on a 64 bit system.
        if !taken {
            ident
        } else {
            loop {
                let possible_ident = self.generate_identity();
                if self.callback_queue.contains_key(&possible_ident) {
                    break possible_ident;
                }
            }
        }
    }

    /// Adds a callback to the queue and returns the key
    fn add_callback(&mut self, ident: usize, cb: impl FnOnce(Js) + 'static) {
        let boxed_cb = Box::new(cb);
        self.callback_queue.insert(ident, boxed_cb);
    }

    pub fn register_io(&mut self, token: usize, cb: impl FnOnce(Js) + 'static) {
        self.add_callback(token, cb);

        print(format!("Event with id: {} registered.", token));
        self.epoll_event_cb_map.insert(token as i64, token);
        self.pending_events += 1;
        self.epoll_pending_events += 1;
    }

    pub fn register_work(
        &mut self,
        task: impl Fn() -> Js + Send + 'static,
        kind: ThreadPoolEventKind,
        cb: impl FnOnce(Js) + 'static,
    ) {
        let callback_id = self.generate_cb_identity();
        self.add_callback(callback_id, cb);

        let event = Event {
            task: Box::new(task),
            callback_id,
            kind,
        };

        // we are not going to implement a real scheduler here, just a LIFO queue
        let available = self.get_available_thread();
        self.thread_pool[available].sender.send(event).expect("register work");
        self.pending_events += 1;
    }

    fn set_timeout(&mut self, ms: u64, cb: impl Fn(Js) + 'static) {
        // Is it theoretically possible to get two equal instants? If so we'll have a bug...
        let now = Instant::now();
        let cb_id = self.generate_cb_identity();
        self.add_callback(cb_id, cb);
        let timeout = now + Duration::from_millis(ms);
        self.timers.insert(timeout, cb_id);
        self.pending_events += 1;
        print(format!("Registered timer event id: {}", cb_id));
    }
}

pub fn set_timeout(ms: u64, cb: impl Fn(Js) + 'static) {
    let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
    rt.set_timeout(ms, cb);
}

// ===== THIS IS PLUGINS CREATED IN C++ FOR THE NODE RUNTIME OR PART OF THE RUNTIME ITSELF =====
// The pointer dereferencing of our runtime is not striclty needed but is mostly for trying to
// emulate a bit of the same feeling as when you use modules in javascript. We could pass the runtime in
// as a reference to our startup function.

struct Crypto;
impl Crypto {
    fn encrypt(n: usize, cb: impl Fn(Js) + 'static + Clone) {
        let work = move || {
            fn fibonacchi(n: usize) -> usize {
                match n {
                    0 => 0,
                    1 => 1,
                    _ => fibonacchi(n - 1) + fibonacchi(n - 2),
                }
            }

            let fib = fibonacchi(n);
            Js::Int(fib)
        };

        let rt = unsafe { &mut *RUNTIME };
        rt.register_work(work, ThreadPoolEventKind::Encrypt, cb);
    }
}

struct Fs;
impl Fs {
    fn read(path: &'static str, cb: impl Fn(Js) + 'static) {
        let work = move || {
            // Let's simulate that there is a very large file we're reading allowing us to actually
            // observe how the code is executed
            thread::sleep(std::time::Duration::from_secs(1));
            let mut buffer = String::new();
            fs::File::open(&path)
                .unwrap()
                .read_to_string(&mut buffer)
                .unwrap();
            Js::String(buffer)
        };
        let rt = unsafe { &mut *RUNTIME };
        rt.register_work(work, ThreadPoolEventKind::FileRead, cb);
    }
}

struct Http;
impl Http {
    pub fn http_get_slow(url: &str, delay_ms: u32, cb: impl Fn(Js) + 'static + Clone) {
        let rt: &mut Runtime = unsafe { &mut *RUNTIME };
        // Don't worry, http://slowwly.robertomurray.co.uk is a site for simulating a delayed
        // response from a server. Perfect for our use case.
        let mut stream: minimio::TcpStream =
            minimio::TcpStream::connect("slowwly.robertomurray.co.uk:80").unwrap();
        let request = format!(
            "GET /delay/{}/url/http://{} HTTP/1.1\r\n\
             Host: slowwly.robertomurray.co.uk\r\n\
             Connection: close\r\n\
             \r\n",
            delay_ms, url
        );

        stream
            .write_all(request.as_bytes())
            .expect("Error writing to stream");

        let token = rt.generate_cb_identity();
        rt.epoll_registrator
            .register(&mut stream, token, minimio::Interests::readable())
            .unwrap();

        let wrapped = move |_n| {
            let mut stream = stream;
            let mut buffer = String::new();
            stream
                .read_to_string(&mut buffer)
                .expect("Stream read error");
            cb(Js::String(buffer));
        };

        rt.register_io(token, wrapped);
    }
}

fn print(t: impl std::fmt::Display) {
    println!("Thread: {}\t {}", current(), t);
}

fn print_content(t: impl std::fmt::Display, descr: &str) {
    println!(
        "\n===== THREAD {} START CONTENT - {} =====",
        current(),
        descr.to_uppercase()
    );

    let content = format!("{}", t); 
    let lines = content.lines().take(2);
    let main_cont: String = lines.map(|l| format!("{}\n", l)).collect();
    let opt_location = content.find("Location");
    let opt_location = opt_location.map(|loc| {
        content[loc..]
        .lines()
        .nth(0)
        .map(|l| format!("{}\n",l))
        .unwrap_or(String::new())
    });    

    println!(
        "{}{}... [Note: Abbreviated for display] ...",
        main_cont,
        opt_location.unwrap_or(String::new())
    );
    println!("===== END CONTENT =====\n");
}

fn current() -> String {
    thread::current().name().unwrap().to_string()
}
