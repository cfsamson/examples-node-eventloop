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
    print("Registering immediate timeout 3");
    set_timeout(0, |_res| {
        print("Immediate3 timed out");
    });
    // let's read the file again and display the text
    print("Second call to read test.txt");
    Fs::read("test.txt", |result| {
        let text = result.into_string().unwrap();
        let len = text.len();
        print(format!("Second count: {} characters.", len));

        // aaand one more time but not in parallell.
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
    Io::http_get_slow("http//www.google.com", 2000, |result| {
        let result = result.into_string().unwrap();
        print_content(result.trim(), "web call");
    });
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
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use minimio;

static mut RUNTIME: usize = 0;

struct Event {
    task: Box<dyn Fn() -> Js + Send + 'static>,
    callback_id: usize,
    kind: EventKind,
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

pub struct Runtime {
    thread_pool: Box<[NodeThread]>,
    available: Vec<usize>,
    callback_queue: HashMap<usize, Box<dyn FnOnce(Js)>>,
    next_tick_callbacks: Vec<(usize, Js)>,
    identity_token: usize,
    pending_events: usize,
    threadp_reciever: Receiver<(usize, usize, Js)>,
    epoll_reciever: Receiver<usize>,
    epoll_pending_events: usize,
    epoll_event_cb_map: HashMap<i64, usize>,
    timers: BTreeMap<Instant, usize>,
    epoll_registrator: minimio::Registrator,
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
                        print(format!("recived a task of type: {}", event.kind));
                        let res = (event.task)();
                        print(format!("finished running a task of type: {}.", event.kind));
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
        let mut poll = minimio::Poll::new().expect("Error creating epoll queue");
        let registrator = poll.registrator();
        thread::Builder::new()
            .name("epoll".to_string())
            .spawn(move || {
                let mut events = minimio::Events::with_capacity(1024);
                loop {
                    match poll.poll(&mut events) {
                        Ok(v) if v > 0 => {
                            for i in 0..v {
                                let event = events.get_mut(i).expect("No events in event list.");
                                print(format!("epoll event {} is ready", event.id().value()));
                                epoll_sender.send(event.id().value() as usize).unwrap();
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
            epoll_pending_events: 0,
            epoll_event_cb_map: HashMap::new(),
            timers: BTreeMap::new(),
            epoll_registrator: registrator,
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

            // ===== 2. TIMERS =====
            self.effectuate_timers(&mut timers_to_remove);

            // NOT PART OF LOOP, JUST FOR US TO SEE WHAT TICK IS EXCECUTING
            if !self.next_tick_callbacks.is_empty() {
                print(format!("===== TICK {} =====", ticks));
            }

            // ===== 2. CALLBACKS =====
            self.run_callbacks();

            // ===== 3. IDLE/PREPARE =====
            // we won't use this

            // ===== 4. POLL =====
            self.process_epoll_events();
            self.process_threadpool_events();

            // ===== 5. CHECK =====
            // an set immidiate function could be added pretty easily but we won't do that here

            // ===== 6. CLOSE CALLBACKS ======
            // Release resources, we won't do that here, it's just another "hook" for our "extensions"
            // to use. We release in every callback instead

            // Let the OS have a time slice of our thread so we don't busy loop
            // this could be dynamically set depending on requirements or load.
            thread::park_timeout(std::time::Duration::from_millis(1));
        }
        print("FINISHED");
    }

    fn effectuate_timers(&mut self, timers_to_remove: &mut Vec<Instant>) {
        self.timers
            .range(..=Instant::now())
            .for_each(|(k, _)| timers_to_remove.push(*k));

        while let Some(key) = timers_to_remove.pop() {
            let callback_id = self.timers.remove(&key).unwrap();
            self.next_tick_callbacks.push((callback_id, Js::Undefined));
        }
    }

    fn run_callbacks(&mut self) {
        while let Some((callback_id, data)) = self.next_tick_callbacks.pop() {
            let cb = self.callback_queue.remove(&callback_id).unwrap();
            cb(data);
            self.pending_events -= 1;
        }
    }

    fn process_epoll_events(&mut self) {
        while let Ok(event_id) = self.epoll_reciever.try_recv() {
            let id = self
                .epoll_event_cb_map
                .get(&(event_id as i64))
                .expect("Event not in event map.");
            let callback_id = *id;
            self.epoll_event_cb_map.remove(&(event_id as i64));

            self.next_tick_callbacks.push((callback_id, Js::Undefined));
            self.epoll_pending_events -= 1;
        }
    }

    fn process_threadpool_events(&mut self) {
        while let Ok((thread_id, callback_id, data)) = self.threadp_reciever.try_recv() {
            self.next_tick_callbacks.push((callback_id, data));
            self.available.push(thread_id);
        }
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
        // this is the happy path
        let boxed_cb = Box::new(cb);
        self.callback_queue.insert(ident, boxed_cb);
    }

    pub fn register_io(&mut self, token: usize, cb: impl FnOnce(Js) + 'static) {
        self.add_callback(token, cb);

        // if no ident is set, set it equal to cb_id + 1 000 000
        // if event.ident == 0 {
        //     event.ident = cb_id as u64 + 1_000_000;
        // }
        print(format!("Event with id: {} registered.", token));
        self.epoll_event_cb_map.insert(token as i64, token);
        self.pending_events += 1;
        self.epoll_pending_events += 1;
        // self.epoll_starter
        //     .send(self.epoll_pending)
        //     .expect("Sending to epoll_starter.");
    }

    pub fn register_work(
        &mut self,
        task: impl Fn() -> Js + Send + 'static,
        kind: EventKind,
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
        let available = self.schedule();
        self.thread_pool[available].sender.send(event).unwrap();
        self.pending_events += 1;
    }

    fn set_timeout(&mut self, ms: u64, cb: impl Fn(Js) + 'static) {
        // Is it theoretically possible to get two equal instants? If so we will have a bug...
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

struct Io;
impl Io {
    pub fn http_get_slow(url: &str, delay_ms: u32, cb: impl Fn(Js) + 'static + Clone) {
        let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
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
        // stream
        //     .set_nonblocking(true)
        //     .expect("set_nonblocking call failed");
        // let fd = stream.as_raw_fd();

        let token = rt.generate_cb_identity();
        rt.epoll_registrator
            .register(&mut stream, token, minimio::Interests::readable())
            .unwrap();

        let wrapped = move |_n| {
            let mut stream = stream;
            // we do this to prevent getting an error if the status somehow changes between the epoll
            // and our read. In a real implementation this should be handled by re-register the event
            // to the epoll queue for example or just accept that there might be a very small amount
            // of blocking happening here (it might even be more costly to re-register the task)
            // stream
            //     .set_nonblocking(false)
            //     .expect("Error setting stream to blocking.");
            let mut buffer = String::new();
            stream
                .read_to_string(&mut buffer)
                .expect("Error reading from stream.");
            cb(Js::String(buffer));
        };

        rt.register_io(token, wrapped);
    }
}

// URl is in www.google.com format, i.e. only the host name, we can't
// request paths at this point
// pub fn http_get(url: &str, cb: impl Fn(Js) + 'static + Clone) {
//     let url_port = format!("{}:80", url);
//     let mut stream: TcpStream = TcpStream::connect(&url_port).unwrap();
//     let request = format!(
//         "GET / HTTP/1.1\r\n\
//          Host: {}\r\n\
//          Connection: close\r\n\
//          \r\n",
//         url
//     );

//     stream
//         .write_all(request.as_bytes())
//         .expect("Error writing to stream");
//     stream
//         .set_nonblocking(true)
//         .expect("set_nonblocking call failed");
//     let fd = stream.as_raw_fd();

//     //let event = minimio::event_read(fd);

//     let wrapped = move |_n| {
//         let mut stream = stream;
//         let mut buffer = String::new();
//         // we do this to prevent getting an error if the status somehow changes between the epoll
//         // and our read. In a real implementation this should be handled by re-register the event
//         // to the epoll queue for example or just accept that there might be a very small amount
//         // of blocking happening here (it might even be more costly to re-register the task)
//         stream
//             .set_nonblocking(false)
//             .expect("Error setting stream to blocking.");
//         stream
//             .read_to_string(&mut buffer)
//             .expect("Error reading from stream.");
//         // The way we do this we know it's a redirect so we grab the location header and
//         // get that webpage instead
//         cb(Js::String(buffer));
//     };

//     let rt: &mut Runtime = unsafe { &mut *(RUNTIME as *mut Runtime) };
//     rt.register_io(event, wrapped);
// }
