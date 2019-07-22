/// Think of this function as the javascript program you have written
fn javascript() {
    p("First call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        p(format!("First count: {} characters.", len));

        p("I want to encrypt something.");
        Crypto::encrypt(text.len(), |result| {
            let n = result.into_int().unwrap();
            p(format!("\"Encrypted\" number is: {}", n));
        })
    });

    p("Registering immediate timeout 1");
    Io::timeout(0, |_res| {
        p("Immediate1 timed out");
    });
    p("Registering immediate timeout 2");
    Io::timeout(0, |_res| {
        p("Immediate2 timed out");
    });
    p("Registering immediate timeout 3");
    Io::timeout(0, |_res| {
        p("Immediate3 timed out");
    });

    // let's read the file again and display the text
    p("Second call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        p(format!("Second count: {} characters.", len));

        // aaand one more time but not in parallell.
        p("Third call to read test.txt");
        Fs::read("test.txt", |result| {
            let text = result.into_string().unwrap();
            p_content(&text, "file read");
        });
    });

    p("Registering a 3000 ms timeout");
    set_timeout(3000, |_res| {
        p("3000ms timer timed out");
        set_timeout(500, |_res| {
            p("500ms timer(nested) timed out");
        });
    });

    p("Registering a 1000 ms timeout");
    set_timeout(1000,  |_res| {
            p("SETTIMEOUT");
        });

    p("Registering http get request to google.com");
    Io::http_get_slow("http//www.google.com", 2000, |result| {
        let result = result.into_string().unwrap();
        p_content(result.trim(), "web call");
    });
}
fn p(t: impl std::fmt::Display) {
    println!("Thread: {}\t {}", current(), t);
}

fn p_content(t: impl std::fmt::Display, decr: &str) {
    println!(
        "\n===== THREAD {} START CONTENT - {} =====",
        current(),
        decr.to_uppercase()
    );
    println!("{}", t);
    println!("===== END CONTENT =====\n");
}

fn current() -> String {
    thread::current().name().unwrap().to_string()
}

fn main() {
    let mut rt = Runtime::new();
    rt.run(javascript);
}

// ===== THIS IS OUR "NODE LIBRARY" =====
use std::collections::{HashMap, BTreeMap};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Instant, Duration};

mod minimio;

const MAX_EPOLL_EVENTS: usize = 1024;
static mut RUNTIME: usize = 0;

type Callback = Box<FnOnce(Js)>;

struct Event {
    task: Box<Fn() -> Js + Send + 'static>,
    callback_id: usize,
    kind: EventKind,
}

enum EventKind {
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
enum Js {
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

#[derive(Debug)]
struct NodeThread {
    handle: JoinHandle<()>,
    sender: Sender<Event>,
}

struct Runtime {
    thread_pool: Box<[NodeThread]>,
    available: Vec<usize>,
    callback_queue: HashMap<usize, Callback>,
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
    fn new() -> Self {
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
                        p(format!("recived a task of type: {}", event.kind));
                        let res = (event.task)();
                        p(format!("finished running a task of type: {}.", event.kind));
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
                                p(format!("epoll event {} is ready", event.ident));
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

    /// This is the event loop
    fn run(&mut self, f: impl Fn()) {
        let rt_ptr: *mut Runtime = self;
        unsafe { RUNTIME = rt_ptr as usize };

        let mut timers_to_remove = vec![]; // avoid allocating on every loop
        let  mut ticks = 0; // just for us priting out
        // First we run our "main" function
        f();

        

        // The we check that we we still have outstanding tasks in the pipeline
        while self.pending_events > 0 {
            ticks += 1;
            if !self.next_tick_callbacks.is_empty() {
                p(format!("===== TICK {} =====", ticks));
            }

            // ===== TIMERS =====
            self.timers
            .range(..=Instant::now())
            .for_each(|(k,_)| timers_to_remove.push(*k));

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
                self.available.push(thread_id);
            }

             // ===== CHECK =====
             // a set immidiate function could be added pretty easily but we won't do that here

             // ===== CLOSE CALLBACKS ======
             // Release resources, we won't do that here, it's just another "hook" for our "extensions"
             // to use. We release in every callback instead

            // Let the OS have a time slice of our thread so we don't busy loop
            thread::sleep(std::time::Duration::from_millis(1));
        }
        p("FINISHED");
    }

    fn schedule(&mut self) -> usize {
        match self.available.pop() {
            Some(thread_id) => thread_id,
            // We would normally queue this
            None => panic!("Out of threads."),
        }
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

    fn register_io(&mut self, mut event: minimio::Event, cb: impl FnOnce(Js) + 'static) {
        let cb_id = self.add_callback(cb) as i64;

        // if no ident is set, set it equal to cb_id + 1 000 000
        if event.ident == 0 {
            event.ident = cb_id as u64 + 1_000_000;
        }
        p(format!("Event with id: {} registered.", event.ident));
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

    fn register_work(
        &mut self,
        task: impl Fn() -> Js + Send + 'static,
        kind: EventKind,
        cb: impl FnOnce(Js) + 'static,
    ) {
        let callback_id = self.add_callback(cb);

        let event = Event {
            task: Box::new(task),
            callback_id,
            kind,
        };

        // we are not going to implement a real scheduler here, just a LIFO queue
        let available = self.schedule();
        self.thread_pool[available].sender.send(event).unwrap();
        self.pending_events += 1;
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

fn set_timeout(ms: u64, cb: impl Fn(Js) + 'static) {
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

        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_work(work, EventKind::Encrypt, cb);
    }
}

struct Fs;
impl Fs {
    fn read(path: &'static str, cb: impl Fn(Js) + 'static) {
        let work = move || {
            // Let's simulate that there is a very large file we're reading allowing us to actually
            // observe how the code is executed
            thread::sleep(std::time::Duration::from_secs(2));
            let mut buffer = String::new();
            fs::File::open(&path)
                .unwrap()
                .read_to_string(&mut buffer)
                .unwrap();
            Js::String(buffer)
        };
        let rt = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_work(work, EventKind::FileRead, cb);
    }
}

// ===== THIS IS OUR EPOLL/KQUEUE/IOCP LIBRARY =====
use std::net::TcpStream;
use std::os::unix::io::{AsRawFd, RawFd};

struct Io;
impl Io {
    pub fn timeout(ms: u32, cb: impl Fn(Js) + 'static) {
        let event = minimio::event_timeout(i64::from(ms));

        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_io(event, cb);
    }

    pub fn http_get_slow(url: &str, delay_ms: u32, cb: impl Fn(Js) + 'static + Clone) {
        // Don't worry, http://slowwly.robertomurray.co.uk is a site for simulating a delayed
        // response from a server. Perfect for our use case.
        let mut stream: TcpStream = TcpStream::connect("slowwly.robertomurray.co.uk:80").unwrap();
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
        stream
            .set_nonblocking(true)
            .expect("set_nonblocking call failed");
        let fd = stream.as_raw_fd();

        let event = minimio::event_read(fd);

        let wrapped = move |_n| {
            let mut stream = stream;
            // we do this to prevent getting an error if the status somehow changes between the epoll
            // and our read. In a real implementation this should be handled by re-register the event
            // to the epoll queue for example or just accept that there might be a very small amount
            // of blocking happening here (it might even be more costly to re-register the task)
            stream
                .set_nonblocking(false)
                .expect("Error setting stream to blocking.");
            let mut buffer = String::new();
            stream
                .read_to_string(&mut buffer)
                .expect("Error reading from stream.");
            cb(Js::String(buffer));
        };

        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_io(event, wrapped);
    }
    /// URl is in www.google.com format, i.e. only the host name, we can't
    /// request paths at this point
    pub fn http_get(url: &str, cb: impl Fn(Js) + 'static + Clone) {
        let url_port = format!("{}:80", url);
        let mut stream: TcpStream = TcpStream::connect(&url_port).unwrap();
        let request = format!(
            "GET / HTTP/1.1\r\n\
             Host: {}\r\n\
             Connection: close\r\n\
             \r\n",
            url
        );

        stream
            .write_all(request.as_bytes())
            .expect("Error writing to stream");
        stream
            .set_nonblocking(true)
            .expect("set_nonblocking call failed");
        let fd = stream.as_raw_fd();

        let event = minimio::event_read(fd);

        let wrapped = move |_n| {
            let mut stream = stream;
            let mut buffer = String::new();
            // we do this to prevent getting an error if the status somehow changes between the epoll
            // and our read. In a real implementation this should be handled by re-register the event
            // to the epoll queue for example or just accept that there might be a very small amount
            // of blocking happening here (it might even be more costly to re-register the task)
            stream
                .set_nonblocking(false)
                .expect("Error setting stream to blocking.");
            stream
                .read_to_string(&mut buffer)
                .expect("Error reading from stream.");
            // The way we do this we know it's a redirect so we grab the location header and
            // get that webpage instead
            cb(Js::String(buffer));
        };

        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
        rt.register_io(event, wrapped);
    }
}
